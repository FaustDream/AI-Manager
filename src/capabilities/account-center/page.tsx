// 账号中心：八列列定义驱动 + 拖动排序 + 列宽持久化 + 操作列 + 详情面板。
// 行 key = iid（四处共用的主键）；界面任何位置不渲染 uid（契约 17）。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useMemo, useRef, useState } from 'react';
import { ChevronDown, ChevronsUpDown, ChevronUp } from 'lucide-react';
import { errKind, errText } from '@/lib/ipc';
import { Input, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { Button, cn } from '@/components/ui/button';
import { toast } from '@/components/ui/toast';
import { ICONS } from '@/components/ui/icons';
import { fmtMs } from '@/lib/format';
import { matchesQuery, nextSort, sortRows, type SortState, type SortValue } from '@/lib/sort';
import {
  FEATURE_CHECKIN,
  FEATURE_RESTORE,
  FEATURE_SWITCH,
  featurePlatform,
  usePlatformFeatures,
} from '@/lib/features';
import type { AccountView } from '@/lib/types';
import { COLUMNS, WEB_LOGIN_SOURCE, webLoginUsable, type Cell } from './columns';
import { AccountDetailDialog } from './detail';
import { OPS_WIDTH, RestoreDialog, RowActions, SwitchDialog } from './actions';
import { AddAccountDialog, AccountToolbar } from './toolbar';
import { moveBy, useRowDrag } from './use-row-drag';

/// 每次进入账号中心补刷一轮余额：积分查询的唯一自动触发点。

/** 昵称旁的徽章，两种互斥：这行是客户端当前登录的号 / 这行只有网页登录的凭据 */
type Badge = { text: string; tone: 'ok' | 'muted'; title: string };

function CellView({
  cell,
  badge,
  onOpen,
}: {
  cell: Cell;
  badge?: Badge;
  /** 给了就渲染成链接样式的可点文本（目前只有昵称列）：整行不再承担打开详情的职责 */
  onOpen?: () => void;
}) {
  // 图标由列定义给名字，这里才取组件——列定义是纯数据，不碰 JSX
  const Icon = cell.icon ? ICONS[cell.icon] : null;
  return (
    <span
      title={cell.title}
      className={
        cell.tone === 'ok' ? 'text-ok' : cell.tone === 'warn' ? 'text-warn' : cell.tone === 'bad' ? 'text-bad' : cell.tone === 'muted' ? 'text-muted' : ''
      }
    >
      <span className="flex min-w-0 items-center gap-1 leading-tight">
        {Icon && <Icon size={13} className="shrink-0" />}
        {cell.text && onOpen ? (
          // 经典超链接样式：默认带下划线 + 手型光标，明确这是唯一可点开详情的入口
          <button
            type="button"
            onClick={onOpen}
            title={cell.title ? `打开账号详情 · ${cell.title}` : `打开「${cell.text}」的详情`}
            className="min-w-0 cursor-pointer truncate text-left underline underline-offset-2 transition-colors duration-[var(--dur-1)] hover:text-accent"
          >
            {cell.text}
          </button>
        ) : (
          cell.text && <span className="truncate">{cell.text}</span>
        )}
        {badge && (
          <span
            className={cn(
              'shrink-0 rounded-full border px-1.5 py-0.5 text-xs leading-none',
              badge.tone === 'muted'
                ? 'border-line bg-card text-muted'
                : 'border-ok/40 bg-ok/10 text-ok',
            )}
            title={badge.title}
          >
            {badge.text}
          </span>
        )}
      </span>
      {cell.sub && <span className="block truncate text-xs leading-tight text-muted">{cell.sub}</span>}
    </span>
  );
}

/**
 * 各列的可排序取值：返回 `null` 表示「没有这个值」，排序时沉底（不占第一屏）。
 *
 * 只用真正能比较的字段：`login` 用到期时刻（不是「剩几天」那个派生文案），
 * `checkin` 用三档枚举（不需要签到 < 未签 < 已签），这样点一次表头就能把
 * 「今天该签还没签的」排到最上面。
 */
const SORT_VALUE: Record<string, (row: AccountView) => SortValue> = {
  platform: (row) => row.platform_label,
  name: (row) => row.name,
  login: (row) => row.expires_at,
  credit: (row) => row.balance,
  expiring: (row) => row.expiring_10d,
  checkin: (row) => (row.requires_checkin ? (row.checked_in === true ? 2 : 1) : 0),
};

export default function AccountCenterPage() {
  const qc = useQueryClient();
  const accounts = useQuery<AccountView[]>({
    queryKey: ['accounts'],
    queryFn: () => queryCmd('account_list'),
  });
  const current = useQuery<string[]>({
    queryKey: ['current'],
    queryFn: () => queryCmd('account_current'),
  });
  // 现在能被切到客户端的账号：网页登录的账号没有登录态档案，切按钮直接禁用
  const switchable = useQuery<string[]>({
    queryKey: ['switchable'],
    queryFn: () => queryCmd('account_switchable'),
  });

  const [selected, setSelected] = useState<string | null>(null);
  const [widths, setWidths] = useState<Record<string, number>>({});
  const [addOpen, setAddOpen] = useState(false);
  const [switchTarget, setSwitchTarget] = useState<AccountView | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<AccountView | null>(null);
  // 切换成功的行留一小段高光：动画结束就清掉，行回到普通的「当前账号」底色
  const [switchedIid, setSwitchedIid] = useState<string | null>(null);
  // 查询失败是运行时状态：接口这轮报错就标红，成功后自动消失，不落库
  const [queryErrors, setQueryErrors] = useState<Record<string, string>>({});
  const rows = accounts.data ?? [];

  // ---- 筛选与排序（只看这一次，不落库：列表顺序的持久化仍归拖拽管）----
  const [query, setQuery] = useState('');
  const [platformFilter, setPlatformFilter] = useState('all');
  const [stateFilter, setStateFilter] = useState<'all' | 'expired' | 'expiring' | 'unchecked'>('all');
  const [sort, setSort] = useState<SortState | null>(null);

  const filtered = useMemo(() => {
    const now = Date.now();
    return rows.filter((row) => {
      if (platformFilter !== 'all' && featurePlatform(row.platform, row.iid) !== platformFilter) {
        return false;
      }
      if (stateFilter === 'expired' && !(row.expires_at !== null && row.expires_at <= now)) return false;
      if (
        stateFilter === 'expiring' &&
        !(row.expires_at !== null && row.expires_at > now && row.expires_at - now <= 7 * 86400_000)
      ) {
        return false;
      }
      if (stateFilter === 'unchecked' && !(row.requires_checkin && row.checked_in !== true)) return false;
      return matchesQuery([row.name, row.platform_label, row.iid, row.note], query);
    });
  }, [rows, platformFilter, stateFilter, query]);

  // 没排序时就是后端保存的 sort_order：用户手动拖到哪，下次打开还在哪
  const visible = useMemo(
    () => sortRows(filtered, sort, (row, key) => SORT_VALUE[key]?.(row)),
    [filtered, sort],
  );
  const filtering = query.trim() !== '' || platformFilter !== 'all' || stateFilter !== 'all';

  // 平台下拉只列当前真正存在的平台：空着一半的下拉框只是噪音
  const platforms = useMemo(() => {
    const map = new Map<string, string>();
    for (const row of rows) map.set(featurePlatform(row.platform, row.iid), row.platform_label);
    return [...map.entries()];
  }, [rows]);

  const selectedRow = rows.find((r) => r.iid === selected) ?? null;
  const currentSet = useMemo(() => new Set(current.data ?? []), [current.data]);
  const switchableSet = useMemo(() => new Set(switchable.data ?? []), [switchable.data]);
  // 哪个软件显示哪个入口（切换 / 恢复对话 / 签到）由设置页决定，页面不再写平台名单
  const features = usePlatformFeatures();

  // 「添加账号」里每个平台已有几个号：WorkBuddy 国内/国际共用 platform 键，
  // 按 iid 的 @intl 后缀拆开，否则两版会被算成同一个平台。
  const platformCounts = useMemo(() => {
    const out: Record<string, number> = {};
    for (const row of rows) {
      const key =
        row.platform === 'workbuddy' && row.iid.endsWith('@intl')
          ? 'workbuddy_intl'
          : row.platform;
      out[key] = (out[key] ?? 0) + 1;
    }
    return out;
  }, [rows]);

  // 从总览等页面跳过来时带着要看的 iid：挂载后直接选中那一行。
  // 用 sessionStorage 而不是 URL 参数：壳层没有地址栏，页面间传值只活这一次。
  useEffect(() => {
    const iid = sessionStorage.getItem('aim.focusAccount');
    if (!iid) return;
    sessionStorage.removeItem('aim.focusAccount');
    setSelected(iid);
  }, []);

  // 每次进入账号中心补刷一轮余额：积分查询的唯一自动触发点。
  // 不挂后台轮询，也不在模块层做节流——用户点进来就是要看最新数字，一次进入打一轮。
  // 用 ref 保证同一次挂载只跑一次（React StrictMode 会双跑 effect）。
  const creditsLooked = useRef(false);
  useEffect(() => {
    if (creditsLooked.current || rows.length === 0) return;
    const iids = rows.filter((r) => r.has_secret && r.enabled).map((r) => r.iid);
    if (iids.length === 0) {
      creditsLooked.current = true;
      return;
    }
    creditsLooked.current = true;
    void runCmd('checkin_credits', { iids, trigger: 'user' }, { onError: () => {} }).then(
      (out) => {
        if (out === undefined) return;
        void qc.invalidateQueries({ queryKey: ['accounts'] });
        void qc.invalidateQueries({ queryKey: ['detail'] });
      },
    );
  }, [rows, qc]);

  // ---- 列宽：持久化在 kv ui.columns ----
  useEffect(() => {
    queryCmd('settings_get', { key: 'ui.columns' })
      .then((saved) => saved && setWidths(saved as Record<string, number>))
      .catch(() => {});
  }, []);
  const saveWidths = (next: Record<string, number>) => {
    setWidths(next);
    // 列宽是纯显示偏好，写失败静默即可；走 runCmd 统一入口，不自拼 catch
    void runCmd('settings_set', { key: 'ui.columns', value: next }, { onError: () => {} });
  };

  // ---- 列宽拖拽 ----
  const resizing = useRef<{ key: string; startX: number; startW: number } | null>(null);
  const widthsRef = useRef(widths);
  widthsRef.current = widths;
  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!resizing.current) return;
      const { key, startX, startW } = resizing.current;
      const col = COLUMNS.find((c) => c.key === key);
      const next = Math.max(col?.min ?? 60, startW + e.clientX - startX);
      setWidths((w) => ({ ...w, [key]: next }));
    };
    const onUp = () => {
      if (resizing.current) {
        // 从 ref 取最新值：闭包里的 widths 是 effect 建立时的旧对象
        saveWidths(widthsRef.current);
        resizing.current = null;
        document.body.classList.remove('resizing');
      }
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
    return () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    // onMove/onUp 全走 ref，不依赖渲染期值：只在挂载时装一次
  }, []);

  // 慢操作的按钮加载态：刷积分/切换/恢复要等几秒，不给 busy 态看起来就像没点上
  const { busy, run: runBusy } = useBusy();
  // 正在切换的那一行（busy 的 key 形如 switch:<iid>）：光环呼吸，成功后再留一段收尾高光
  const switchingIid = busy?.startsWith('switch:') ? busy.slice('switch:'.length) : null;
  useEffect(() => {
    if (!switchedIid) return;
    const timer = window.setTimeout(() => setSwitchedIid(null), 900);
    return () => window.clearTimeout(timer);
  }, [switchedIid]);

  // ---- 拖动排序：指针逻辑在 use-row-drag，这里只负责持久化 ----
  // 排序生效时不允许拖拽：拖出来的是「排序后的视图顺序」，写回后端会把人工顺序搅乱
  const { dragging, dropAt, startDrag } = useRowDrag(visible, sort ? 'sorted' : busy, async (iids) => {
    if ((await runCmd('account_reorder', { iids })) === undefined) return;
    qc.invalidateQueries({ queryKey: ['accounts'] });
  });

  const widthOf = (key: string, fallback: number) => widths[key] ?? fallback;

  /** 键盘上移/下移（P2-11）：与拖拽同一条命令；到边界时 moveBy 给 null，不发请求。 */
  const moveRow = async (iid: string, delta: number) => {
    if (busy) return;
    const iids = moveBy(
      visible.map((r) => r.iid),
      iid,
      delta,
    );
    if (!iids) return;
    if ((await runCmd('account_reorder', { iids })) === undefined) return;
    qc.invalidateQueries({ queryKey: ['accounts'] });
  };

  // 表格最小宽度 = 拖动列 + 各列实际宽度 + 操作列。跟着列宽/删列自动变，
  // 不再写死一个数字（写死了删掉一列就多出一截空白滚动区）。
  const minWidth = useMemo(
    () =>
      24 +
      OPS_WIDTH +
      COLUMNS.reduce((sum, col) => sum + (widths[col.key] ?? col.width), 0),
    [widths],
  );

  // 窄窗口才把操作列钉在右侧：宽窗口表格本来就不滚，常驻 sticky 会多出一条
  // 没必要的分隔线。观察表格自身宽度变化，列宽拖大时也能跟着切换。
  const tableBox = useRef<HTMLTableElement | null>(null);
  const [narrow, setNarrow] = useState(false);
  useEffect(() => {
    const table = tableBox.current;
    const wrap = table?.parentElement;
    if (!table || !wrap) return;
    const check = () =>
      setNarrow(table.getBoundingClientRect().width > wrap.clientWidth + 1);
    check();
    const ro = new ResizeObserver(check);
    ro.observe(table);
    ro.observe(wrap);
    return () => ro.disconnect();
  }, []);

  const refreshCredits = () =>
    runBusy('credits', async () => {
      const iids = rows.filter((r) => r.has_secret && r.enabled).map((r) => r.iid);
      if (iids.length === 0) {
        toast('没有可刷新的账号（先添加账号）');
        return;
      }
      const started = performance.now();
      const out = await runCmd('checkin_credits', { iids });

      if (out === undefined) return;
      // 只有后端标了 error 才算失败：成功但没拿到数值不能标红。
      // 运行时状态，下次刷新成功自动消失，不进库。
      const errors: Record<string, string> = {};
      for (const [iid, snapshot] of out) {
        if (snapshot.error) {
          errors[iid] = snapshot.error;
        }
      }
      setQueryErrors(errors);
      // 等列表与详情真正重取完再报「完成」：否则 toast 说刷新完了，屏幕还是旧值
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['accounts'] }),
        qc.invalidateQueries({ queryKey: ['detail'] }),
      ]);
      const failed = Object.keys(errors).length;
      const elapsed = fmtMs(Math.round(performance.now() - started));
      toast(
        failed === 0
          ? `刷新完成：${out.length} 个账号全部成功 · ${elapsed}`
          : `刷新完成：${out.length} 个账号，${failed} 个查询失败（原因见列表红字与日志）· ${elapsed}`,
      );
    });

  // 批量刷新账号：重扫客户端，把每个账号的最新凭据、到期时间写回本地库，
  // 并同步「当前登录 / 可切换」标记。余额不在这一步查（那是「积分」按钮）。
  // 不管是否过期都会刷——平台侧的有效期可能已经被客户端续过。
  const batchRefreshCredentials = () =>
    runBusy('refresh-credentials', async () => {
      const iids = visible.map((r) => r.iid);
      if (iids.length === 0) {
        toast('当前筛选下没有账号');
        return;
      }
      const report = await runCmd('account_refresh_credentials', { iids });
      if (report === undefined) return;
      const parts = [
        report.refreshed > 0 ? `更新 ${report.refreshed}` : '',
        report.up_to_date > 0 ? `已是最新 ${report.up_to_date}` : '',
        report.need_reauth > 0 ? `需重新授权 ${report.need_reauth}` : '',
        report.unsupported > 0 ? `不支持 ${report.unsupported}` : '',
        report.failed > 0 ? `失败 ${report.failed}` : '',
      ].filter(Boolean);
      const head = `刷新完成：${report.total} 个账号 · ${parts.join(' · ') || '无需处理'} · ${fmtMs(report.ms)}`;
      toast(
        report.errors.length === 0
          ? head
          : `${head}\n${report.errors.slice(0, 2).join('\n')}${
              report.errors.length > 2 ? `\n…另 ${report.errors.length - 2} 条见日志` : ''
            }`,
      );
      // 重扫也会更新「当前登录」与可切换范围：一起重取，别等下一次进页面
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['accounts'] }),
        qc.invalidateQueries({ queryKey: ['detail'] }),
        qc.invalidateQueries({ queryKey: ['current'] }),
        qc.invalidateQueries({ queryKey: ['switchable'] }),
        qc.invalidateQueries({ queryKey: ['codebuddy-targets'] }),
      ]);
    });

  const batchCheckin = () =>
    runBusy('checkin', async () => {
      // 与「刷新」同一口径：只处理当前筛选出来的账号（按钮说的是「列表里」）
      const iids = visible
        .filter(
          (r) =>
            r.requires_checkin &&
            r.has_secret &&
            r.enabled &&
            r.checked_in !== true &&
            // 设置里关掉某个软件的签到入口后，批量签到也不再带上它
            features.on(featurePlatform(r.platform, r.iid), FEATURE_CHECKIN),
        )
        .map((r) => r.iid);
      if (iids.length === 0) {
        toast('没有需要签到的账号');
        return;
      }
      const started = performance.now();
      const round = await runCmd('checkin_batch', { iids });

      if (round === undefined) return;
      // 签到会改余额与今日标记：列表与详情必须一起重取
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['accounts'] }),
        qc.invalidateQueries({ queryKey: ['detail'] }),
      ]);
      const elapsed = fmtMs(Math.round(performance.now() - started));
      const parts = [
        round.success > 0 ? `成功 ${round.success}` : '',
        round.already > 0 ? `已签 ${round.already}` : '',
        round.failed > 0 ? `失败 ${round.failed}` : '',
        round.skipped > 0 ? `跳过 ${round.skipped}` : '',
      ].filter(Boolean);
      toast(
        `批量签到完成：${parts.join(' · ') || '无需处理'} · ${elapsed}` +
          (round.failed > 0 ? '\n失败详情见「日志 → 执行动作」' : ''),
      );
    });

  // 添加账号的整条链路（关客户端 → 备份登录态 → 打开登录页 → 扫描）都在弹窗里，
  // 页面只负责在扫完后刷新列表——弹窗中途不关，用户才看得到「正在做什么」。
  const afterAddScanned = () => {
    qc.invalidateQueries({ queryKey: ['accounts'] });
    qc.invalidateQueries({ queryKey: ['current'] });
    qc.invalidateQueries({ queryKey: ['switchable'] });
    qc.invalidateQueries({ queryKey: ['facets'] });
    qc.invalidateQueries({ queryKey: ['sessions'] });
  };

  // ---- 操作列 ----
  /** 恢复/切换只改了客户端库；本地会话索引要重扫一次才会换成新账号名。
   *  扫描是增量的，客户端库指纹变了才重扫 WorkBuddy 这一个来源。 */
  const rescanSessions = () => {
    queryCmd('session_scan').catch(() => {});
  };

  const doSwitch = () => {
    const target = switchTarget;
    if (!target) return;
    setSwitchTarget(null);
    void runBusy(`switch:${target.iid}`, async () => {
      try {
        const r = await queryCmd('account_switch', { iid: target.iid });
        setSwitchedIid(target.iid);
        toast(r.message);
        // 切换会改写客户端登录态，也会把旧账号的会话关联过来：列表、当前账号、会话页都要刷
        qc.invalidateQueries({ queryKey: ['current'] });
        qc.invalidateQueries({ queryKey: ['switchable'] });
        qc.invalidateQueries({ queryKey: ['accounts'] });
        qc.invalidateQueries({ queryKey: ['detail'] });
        qc.invalidateQueries({ queryKey: ['sessions'] });
        qc.invalidateQueries({ queryKey: ['facets'] });
        if (target.platform === 'workbuddy') rescanSessions();
      } catch (e) {
        // 切换失败代价高，kind 给界面机会说清下一步（client_running = 客户端没能自动关掉）
        const kind = errKind(e);
        toast(
          kind === 'client_running'
            ? `客户端没能自动关闭，请从托盘退出后再试。\n${errText(e)}`
            : errText(e),
        );
      }
    });
  };

  const doRestore = () => {
    const target = restoreTarget;
    if (!target) return;
    setRestoreTarget(null);
    void runBusy(`restore:${target.iid}`, async () => {
      try {
        const r = await queryCmd('account_restore_sessions', { iid: target.iid });
        toast(
          r.changed_rows > 0
            ? `已把 ${r.changed_rows} 条历史会话关联到「${target.name}」`
            : '没有需要关联的会话（当前账号已经能看到全部历史）',
        );
        qc.invalidateQueries({ queryKey: ['sessions'] });
        qc.invalidateQueries({ queryKey: ['facets'] });
        rescanSessions();
      } catch (e) {
        const kind = errKind(e);
        toast(kind === 'client_running' ? `请先关闭 WorkBuddy 再恢复会话。\n${errText(e)}` : errText(e));
      }
    });
  };

  const toggleEnabled = (row: AccountView) =>
    runBusy(`enabled:${row.iid}`, async () => {
      const changed = await runCmd('account_set_enabled', {
        iid: row.iid,
        enabled: !row.enabled,
      });
      if (changed === undefined) return;
      toast(`已${row.enabled ? '停用' : '启用'}「${row.name}」`);
      qc.invalidateQueries({ queryKey: ['accounts'] });
    });

  const removeAccount = async (row: AccountView) => {
    // 不可逆动作（同页的会话归档 / 彻底删除都有确认）
    if (!(await confirmAction({ title: '确认操作', message: `从账号库移除「${row.name}」？\n只删本机记录，客户端登录态不动。`, danger: true }))) return;
    if ((await runCmd('account_delete', { iid: row.iid })) === undefined) return;
    toast('已从账号库移除（客户端登录态未动）');
    if (selected === row.iid) setSelected(null);
    qc.invalidateQueries({ queryKey: ['accounts'] });
  };

  return (
    <div className="stagger flex flex-col gap-2.5">
      {/* 动作行 + 筛选行合成一张工具条：两圈边框会看着像两个控件区 */}
      <div className="overflow-hidden rounded-md border border-line bg-card shadow-card">
        <div className="border-b border-line bg-sunken px-2.5 py-2">
        <AccountToolbar
          busy={busy}
          onAdd={() => setAddOpen(true)}
          onRefresh={refreshCredits}
          onRefreshCredentials={batchRefreshCredentials}
          onCheckin={batchCheckin}
        />

        </div>
      {/* 筛选行：搜索 + 平台 + 状态 + 计数。列表是全量拉回来的，过滤与排序都在前端做，
          敲一下就变，不额外打接口。 */}
        <div className="flex flex-wrap items-center gap-1.5 px-2.5 py-2">
          <Input
            className="w-56"
            value={query}
            aria-label="搜索账号"
            placeholder="搜索昵称 / UID / 平台 / 备注"
            onChange={(e) => setQuery(e.target.value)}
          />
          <Select
            className="w-36"
            value={platformFilter}
            aria-label="按平台筛选"
            onChange={(e) => setPlatformFilter(e.target.value)}
          >
            <option value="all">全部平台</option>
            {platforms.map(([key, label]) => (
              <option key={key} value={key}>
                {label}
              </option>
            ))}
          </Select>
          <Select
            className="w-32"
            value={stateFilter}
            aria-label="按状态筛选"
            onChange={(e) => setStateFilter(e.target.value as typeof stateFilter)}
          >
            <option value="all">全部状态</option>
            <option value="expired">已过期</option>
            <option value="expiring">7 天内到期</option>
            <option value="unchecked">今日未签</option>
          </Select>
          {filtering && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setQuery('');
                setPlatformFilter('all');
                setStateFilter('all');
              }}
            >
              清除筛选
            </Button>
          )}
          <span className="flex-1" />
          <span className="text-xs text-muted tabular-nums" title="当前显示 / 全部账号">
            {visible.length} / {rows.length}
          </span>
        </div>
      </div>

      {/* 窄窗口只在表格自己身上横向滚动，操作列 sticky 在右侧始终可点；
          详情面板不跟着横向滚，避免整页左右晃 */}
      <div className="overflow-x-auto rounded-md border border-line bg-card shadow-card">
      <table ref={tableBox} className="w-full table-fixed border-collapse" style={{ minWidth }}>
        <thead className="bg-sunken">
          <tr>
            <th className="w-6 border-b border-line-strong px-1 py-1.5" />
            {COLUMNS.map((col) => {
              const dir = sort?.key === col.key ? sort.dir : null;
              const sortable = SORT_VALUE[col.key] !== undefined;
              return (
              <th
                key={col.key}
                className="relative select-none overflow-hidden whitespace-nowrap border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted"
                style={{ width: widthOf(col.key, col.width) }}
                title={col.headerTitle}
                aria-sort={dir ? (dir === 'asc' ? 'ascending' : 'descending') : sortable ? 'none' : undefined}
              >
                {sortable ? (
                  <button
                    type="button"
                    className={cn(
                      'inline-flex cursor-pointer items-center gap-1 rounded-sm transition-colors duration-[var(--dur-1)] hover:text-fg',
                      dir && 'text-accent',
                    )}
                    title="点击排序：升序 → 降序 → 取消"
                    onClick={() => setSort((current) => nextSort(current, col.key))}
                  >
                    {col.header}
                    {dir === 'asc' ? (
                      <ChevronUp size={12} aria-hidden className="shrink-0" />
                    ) : dir === 'desc' ? (
                      <ChevronDown size={12} aria-hidden className="shrink-0" />
                    ) : (
                      <ChevronsUpDown size={12} aria-hidden className="shrink-0 opacity-30" />
                    )}
                  </button>
                ) : (
                  col.header
                )}
                <span
                  className="absolute bottom-0 right-0 top-0 w-[7px] cursor-col-resize hover:bg-accent/35"
                  title="拖动调宽，双击复位"
                  onMouseDown={(e) => {
                    resizing.current = {
                      key: col.key,
                      startX: e.clientX,
                      startW: widthOf(col.key, col.width),
                    };
                    document.body.classList.add('resizing');
                  }}
                  onDoubleClick={() => saveWidths({ ...widths, [col.key]: col.width })}
                />
              </th>
              );
            })}
            <th
              className={`border-b border-line-strong px-2.5 py-1.5 text-right text-xs font-semibold text-muted ${
                narrow
                  ? 'sticky right-0 z-10 border-l border-line-strong bg-sunken shadow-sticky-edge'
                  : ''
              }`}
              style={{ width: OPS_WIDTH }}
            >
              操作
            </th>
          </tr>
        </thead>
        <tbody>
          {accounts.isLoading && <SkeletonRows rows={4} cols={COLUMNS.length + 2} />}
          {accounts.isError && (
            <tr>
              <td colSpan={COLUMNS.length + 2}>
                <StateBlock
                  kind="error"
                  title="读账号失败"
                  detail={errText(accounts.error)}
                  onRetry={() => accounts.refetch()}
                />
              </td>
            </tr>
          )}
          {!accounts.isLoading && visible.length === 0 && (
            <tr>
              <td colSpan={COLUMNS.length + 2}>
                {filtering ? (
                  <StateBlock
                    kind="empty"
                    title="没有符合条件的账号"
                    detail={`共 ${rows.length} 个账号，当前筛选一个都没匹配上。`}
                  />
                ) : (
                  <StateBlock
                    kind="empty"
                    title="还没有账号"
                    detail="启动时会自动扫描客户端；也可以点「添加账号」登录后入库。"
                  />
                )}
              </td>
            </tr>
          )}
          {visible.map((row) => {
            const isCurrent = currentSet.has(row.iid);
            const canSwitch = features.on(featurePlatform(row.platform, row.iid), FEATURE_SWITCH);
            const canSwitchAccount = switchableSet.has(row.iid);
            // 「仅网页登录」= 这个平台本来能切，但本机没有可写回的登录态。当前登录优先：
            // 客户端此刻就登着这个号时，再挂「仅网页登录」会自相矛盾。
            const nameBadge: Badge | undefined = isCurrent
              ? { text: '当前登录', tone: 'ok', title: '客户端此刻登录的就是这个账号' }
              : row.platform === 'workbuddy' && current.isSuccess && switchable.isSuccess && canSwitch && !canSwitchAccount && row.source === WEB_LOGIN_SOURCE
                ? {
                    text: '仅网页登录',
                    tone: 'muted',
                    title: `凭据来自网页登录：${webLoginUsable(row)}照常可用，` +
                      '只有「切换客户端登录态」做不了。要能切，需在客户端里登录一次这个账号',
                  }
                : undefined;
            const isDragging = dragging === row.iid;
            const dropBefore = dropAt?.iid === row.iid && dropAt.before;
            const dropAfter = dropAt?.iid === row.iid && !dropAt.before;
            return (
              <tr
                key={row.iid}
                data-iid={row.iid}
                // 详情只由昵称链接触发：整行点击会把「选文字 / 看某一列」也当成打开弹窗
                className={cn(
                  'transition-colors duration-[var(--dur-1)]',
                  // 底色只表示「正在看哪一行」：详情面板在表格下方，这是唯一能对上号的线索。
                  // 「客户端当前登录」由昵称旁的徽章表达，不再和选中态抢同一块颜色。
                  selected === row.iid ? 'bg-accent/15 hover:bg-accent/20' : 'hover:bg-hover',
                  switchingIid === row.iid && 'switch-pending',
                  switchedIid === row.iid && 'switch-flash',
                  isDragging && 'opacity-40',
                )}
                style={
                  dropBefore
                    ? { boxShadow: 'inset 0 2px 0 var(--color-accent)' }
                    : dropAfter
                      ? { boxShadow: 'inset 0 -2px 0 var(--color-accent)' }
                      : undefined
                }
              >
                <td
                  className={cn(
                    'px-1 py-1.5 text-center text-muted select-none',
                    // 选中行再来一条左侧竖条：只靠半透明底色在浅色主题下不够醒目
                    selected === row.iid && 'shadow-[inset_2px_0_0_0_var(--color-accent)]',
                  )}
                  style={{ touchAction: 'none' }}
                >
                  {/* 拖拽柄同时是键盘入口（P2-11）：Alt+↑/↓ 走同一个 account_reorder。
                      不在操作列再加两个按钮——那会把刻意收窄过的操作列重新撑宽。 */}
                  <button
                    type="button"
                    className="cursor-grab rounded-sm px-0.5 hover:text-fg"
                    onPointerDown={(e) => startDrag(e, row.iid)}
                    onKeyDown={(e) => {
                      if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
                      e.preventDefault();
                      void moveRow(row.iid, e.key === 'ArrowUp' ? -1 : 1);
                    }}
                    title="拖动排序；键盘：Alt+↑ 上移 / Alt+↓ 下移"
                    aria-label={`调整「${row.name}」的顺序：拖动，或按 Alt+↑ / Alt+↓`}
                  >
                    ⠿
                  </button>
                </td>
                {COLUMNS.map((col) => (
                  <td
                    key={col.key}
                    className="overflow-hidden text-ellipsis whitespace-nowrap border-b border-line px-2.5 py-1.5 align-middle"
                  >
                    <CellView
                      cell={col.cell(row, queryErrors, features.on)}
                      // 昵称旁的徽章：「当前登录」或「仅网页登录」
                      badge={col.key === 'name' ? nameBadge : undefined}
                      // 只有昵称是打开详情的入口；其余列保持纯展示
                      onOpen={col.key === 'name' ? () => setSelected(row.iid) : undefined}
                    />
                  </td>
                ))}
                <td
                  className={`border-b border-line px-2 py-1.5 ${
                    narrow
                      ? `sticky right-0 z-10 border-l border-line shadow-sticky-edge ${
                          selected === row.iid ? 'bg-hover' : 'bg-bg'
                        }`
                      : ''
                  }`}
                >
                  <RowActions
                    row={row}
                    isCurrent={isCurrent}
                    switchable={canSwitchAccount}
                    canSwitch={canSwitch}
                    canRestore={features.on(featurePlatform(row.platform, row.iid), FEATURE_RESTORE)}
                    busy={!!busy}
                    onSwitch={() => setSwitchTarget(row)}
                    onRestore={() => setRestoreTarget(row)}
                    onToggleEnabled={() => void toggleEnabled(row)}
                    onDelete={() => void removeAccount(row)}
                  />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      </div>

      {/* 点昵称即开详情弹窗：概览 / 积分批次 / 领取记录 / 旅行分标签页，不再在表下铺长面板 */}
      {selectedRow && (
        <AccountDetailDialog
          key={selectedRow.iid}
          account={selectedRow}
          isCurrent={currentSet.has(selectedRow.iid)}
          error={queryErrors[selectedRow.iid]}
          onClose={() => setSelected(null)}
        />
      )}

      <AddAccountDialog
        open={addOpen}
        counts={platformCounts}
        onClose={() => setAddOpen(false)}
        onScanned={afterAddScanned}
      />

      <SwitchDialog
        target={switchTarget}
        // 没有登录态档案的账号（网页登录）也让他点开：弹窗里写明为什么切不了、怎么补
        switchable={switchTarget ? switchableSet.has(switchTarget.iid) : true}
        busy={busy?.startsWith('switch:') ?? false}
        onClose={() => setSwitchTarget(null)}
        onConfirm={doSwitch}
      />

      <RestoreDialog
        target={restoreTarget}
        busy={busy?.startsWith('restore:') ?? false}
        onClose={() => setRestoreTarget(null)}
        onConfirm={doRestore}
      />
    </div>
  );
}
