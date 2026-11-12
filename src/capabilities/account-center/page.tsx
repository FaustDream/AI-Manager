// 账号中心：八列列定义驱动 + 拖动排序 + 列宽持久化 + 操作列 + 详情面板。
// 行 key = iid（四处共用的主键）；界面任何位置不渲染 uid（契约 17）。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useMemo, useRef, useState } from 'react';
import { errKind, errText } from '@/lib/ipc';
import { Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { ICONS } from '@/components/ui/icons';
import { fmtMs } from '@/lib/format';
import type { AccountView } from '@/lib/types';
import { attentionRank, COLUMNS, type Cell } from './columns';
import { DetailPanel } from './detail';
import { OPS_WIDTH, RestoreDialog, RowActions, SwitchDialog } from './actions';
import { AddAccountDialog, AccountToolbar } from './toolbar';
import { useRowDrag } from './use-row-drag';

function CellView({ cell }: { cell: Cell }) {
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
        <span className="truncate">{cell.text}</span>
      </span>
      {cell.sub && <span className="block truncate text-xs leading-tight text-muted">{cell.sub}</span>}
    </span>
  );
}

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

  const [selected, setSelected] = useState<string | null>(null);
  const [widths, setWidths] = useState<Record<string, number>>({});
  const [addOpen, setAddOpen] = useState(false);
  const [switchTarget, setSwitchTarget] = useState<AccountView | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<AccountView | null>(null);
  // 查询失败是运行时状态：接口这轮报错就标红，成功后自动消失，不落库
  const [queryErrors, setQueryErrors] = useState<Record<string, string>>({});
  const [sortMode, setSortMode] = useState<'manual' | 'attention'>('manual');
  const rows = accounts.data ?? [];

  // 「需处理优先」是临时排序，不改 sort_order
  const visible = useMemo(
    () =>
      sortMode === 'attention'
        ? [...rows].sort((a, b) => attentionRank(a) - attentionRank(b))
        : rows,
    [rows, sortMode],
  );

  const canDrag = sortMode === 'manual';

  const selectedRow = rows.find((r) => r.iid === selected) ?? null;
  const currentSet = useMemo(() => new Set(current.data ?? []), [current.data]);

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

  // ---- 列宽：持久化在 kv ui.columns（v1 同键） ----
  useEffect(() => {
    queryCmd('settings_get', { key: 'ui.columns' })
      .then((saved) => saved && setWidths(saved as Record<string, number>))
      .catch(() => {});
  }, []);
  const saveWidths = (next: Record<string, number>) => {
    setWidths(next);
    queryCmd('settings_set', { key: 'ui.columns', value: next }).catch(() => {});
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

  // ---- 拖动排序：指针逻辑在 use-row-drag，这里只负责持久化 ----
  const { dragging, dropAt, startDrag } = useRowDrag(visible, canDrag ? busy : 'locked', async (iids) => {
    
      if ((await runCmd('account_reorder', { iids })) === undefined) return;
      qc.invalidateQueries({ queryKey: ['accounts'] });
    
  });

  const widthOf = (key: string, fallback: number) => widths[key] ?? fallback;

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

  // 刷的是「本地库 ↔ 客户端登录态」的一致性：重扫凭据 + 校正到期时间 + 重读当前登录。
  // 与「刷新积分」分开：那个打平台接口拿余额，这个只碰本机数据。
  const refreshState = () =>
    runBusy('refresh-state', async () => {
      const report = await runCmd('account_refresh_all');
      if (report === undefined) return;
      const parts = [
        `${report.total} 个账号`,
        `有凭据 ${report.with_secret}`,
        report.missing_secret > 0 ? `缺凭据 ${report.missing_secret}` : '',
        report.expired > 0 ? `已过期 ${report.expired}` : '',
        report.expiring > 0 ? `临期 ${report.expiring}` : '',
        report.updated > 0 ? `校正到期 ${report.updated}` : '',
      ].filter(Boolean);
      const head = `账号状态已刷新：${parts.join(' · ')} · ${fmtMs(report.ms)}`;
      toast(
        report.errors.length === 0
          ? head
          : `${head}\n${report.errors.slice(0, 2).join('\n')}${
              report.errors.length > 2 ? `\n…另 ${report.errors.length - 2} 条见日志` : ''
            }`,
      );
      // 到期时间、「当前登录」标记都在这一批里更新过：列表、当前账号、详情一起重取
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['accounts'] }),
        qc.invalidateQueries({ queryKey: ['current'] }),
        qc.invalidateQueries({ queryKey: ['detail'] }),
        qc.invalidateQueries({ queryKey: ['codebuddy-targets'] }),
      ]);
    });

  // 批量刷新凭据：对当前筛选出的账号逐个重扫并写回**最新凭据与有效期**。
  // 与「刷新账号状态」的分工：那个是全量重扫 + 校正 + 更新当前登录标记；
  // 这个是针对当前筛选的一批账号，逐个给出「更新了 / 已是最新 / 需重登」的结论。
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
        report.need_relogin > 0 ? `需重登 ${report.need_relogin}` : '',
        report.unsupported > 0 ? `不支持 ${report.unsupported}` : '',
        report.failed > 0 ? `失败 ${report.failed}` : '',
      ].filter(Boolean);
      const head = `刷新凭据完成：${report.total} 个账号 · ${parts.join(' · ') || '无需处理'} · ${fmtMs(report.ms)}`;
      toast(
        report.errors.length === 0
          ? head
          : `${head}\n${report.errors.slice(0, 2).join('\n')}${
              report.errors.length > 2 ? `\n…另 ${report.errors.length - 2} 条见日志` : ''
            }`,
      );
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['accounts'] }),
        qc.invalidateQueries({ queryKey: ['detail'] }),
      ]);
    });

  const batchCheckin = () =>    runBusy('checkin', async () => {
      const iids = rows
        .filter((r) => r.requires_checkin && r.has_secret && r.enabled && r.checked_in !== true)
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

  const batchDiagnose = () =>
    runBusy('diagnose', async () => {
      const iids = visible.filter((r) => r.has_secret && r.enabled).map((r) => r.iid);
      if (iids.length === 0) {
        toast('当前筛选下没有可诊断的账号（需要凭据且已启用）');
        return;
      }
      const result = await runCmd('account_batch_diagnose', { iids });
      if (result === undefined) return;
      const head = `批量诊断：${result.total} 个账号 · 成功 ${result.ok} · 失败 ${result.failed.length}`;
      toast(
        result.failed.length === 0
          ? head
          : `${head}\n${result.failed.slice(0, 2).join('\n')}${
              result.failed.length > 2 ? `\n…另 ${result.failed.length - 2} 条见日志` : ''
            }`,
      );
      qc.invalidateQueries({ queryKey: ['accounts'] });
    });

  const exportList = () =>
    runBusy('export', async () => {
      const path = await runCmd('account_export');
      if (path === undefined) return;
      toast(`账号清单已导出：${path}`);
    });

  // 添加账号的整条链路（关客户端 → 备份登录态 → 打开登录页 → 扫描）都在弹窗里，
  // 页面只负责在扫完后刷新列表——弹窗中途不关，用户才看得到「正在做什么」。
  const afterAddScanned = () => {
    qc.invalidateQueries({ queryKey: ['accounts'] });
    qc.invalidateQueries({ queryKey: ['current'] });
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
        toast(r.message);
        // 切换会改写客户端登录态，也会把旧账号的会话关联过来：列表、当前账号、会话页都要刷
        qc.invalidateQueries({ queryKey: ['current'] });
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

  const table = useMemo(() => COLUMNS, []);

  return (
    <div className="flex flex-col gap-2.5">
      <AccountToolbar
        busy={busy}
        onAdd={() => setAddOpen(true)}
        onRefresh={refreshCredits}
        onRefreshState={refreshState}
        onRefreshCredentials={batchRefreshCredentials}
        onCheckin={batchCheckin}
        onDiagnose={batchDiagnose}
      />

      {/* 导出与排序：只影响展示，不写库 */}
      <div className="flex flex-wrap items-center gap-1.5 text-xs">
        <span className="flex-1" />
        <Button
          size="sm"
          variant="ghost"
          onClick={exportList}
          loading={busy === 'export'}
          disabled={!!busy}
          title="导出当前全部账号（不含凭据），CSV 可直接用表格打开"
        >
          导出清单
        </Button>
        <Select
          value={sortMode}
          title="手动顺序可拖动调整；需处理优先只是临时排序，不改保存的顺序"
          onChange={(e) => setSortMode(e.target.value as 'manual' | 'attention')}
        >
          <option value="manual">手动顺序</option>
          <option value="attention">需处理优先</option>
        </Select>
      </div>

      {/* 窄窗口只在表格自己身上横向滚动，操作列 sticky 在右侧始终可点；
          详情面板不跟着横向滚，避免整页左右晃 */}
      <div className="overflow-x-auto">
      <table ref={tableBox} className="w-full table-fixed border-collapse" style={{ minWidth }}>
        <thead>
          <tr>
            <th className="w-6 border-b border-line-strong px-1 py-1.5" />
            {table.map((col) => (
              <th
                key={col.key}
                className="relative select-none overflow-hidden whitespace-nowrap border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted"
                style={{ width: widthOf(col.key, col.width) }}
              >
                {col.header}
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
            ))}
            <th
              className={`border-b border-line-strong px-2.5 py-1.5 text-right text-xs font-semibold text-muted ${
                narrow
                  ? 'sticky right-0 z-10 border-l border-line-strong bg-bg shadow-[-10px_0_10px_-8px_rgba(16,24,40,0.28)]'
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
                <StateBlock kind="empty" title="还没有账号" detail="启动时会自动扫描客户端；也可以点「添加账号」登录后入库。" />
              </td>
            </tr>
          )}
          {visible.map((row) => {
            const isCurrent = currentSet.has(row.iid);
            const isDragging = dragging === row.iid;
            const dropBefore = dropAt?.iid === row.iid && dropAt.before;
            const dropAfter = dropAt?.iid === row.iid && !dropAt.before;
            return (
              <tr
                key={row.iid}
                data-iid={row.iid}
                onClick={() => setSelected(row.iid)}
                className={`cursor-pointer ${selected === row.iid ? 'bg-hover' : ''} ${
                  isDragging ? 'opacity-40' : ''
                }`}
                style={
                  dropBefore
                    ? { boxShadow: 'inset 0 2px 0 var(--color-accent)' }
                    : dropAfter
                      ? { boxShadow: 'inset 0 -2px 0 var(--color-accent)' }
                      : undefined
                }
              >
                <td
                  className="px-1 py-1.5 text-center text-muted select-none"
                  style={{ cursor: canDrag ? 'grab' : 'default', touchAction: 'none' }}
                  onPointerDown={(e) => {
                    if (canDrag) startDrag(e, row.iid);
                  }}
                  title={canDrag ? '拖动排序' : '切回「手动顺序」且不筛选标签后可拖动'}
                >
                  {canDrag ? '⠿' : '·'}
                </td>
                {table.map((col) => (
                  <td
                    key={col.key}
                    className="overflow-hidden text-ellipsis whitespace-nowrap border-b border-line px-2.5 py-1.5 align-middle"
                  >
                    <CellView cell={col.cell(row, queryErrors)} />
                  </td>
                ))}
                <td
                  className={`border-b border-line px-2 py-1.5 ${
                    narrow
                      ? `sticky right-0 z-10 border-l border-line shadow-[-10px_0_10px_-8px_rgba(16,24,40,0.28)] ${
                          selected === row.iid ? 'bg-hover' : 'bg-bg'
                        }`
                      : ''
                  }`}
                  onClick={(e) => e.stopPropagation()}
                >
                  <RowActions
                    row={row}
                    isCurrent={isCurrent}
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

      <DetailPanel
        account={selectedRow}
        isCurrent={selectedRow ? currentSet.has(selectedRow.iid) : false}
        error={selectedRow ? queryErrors[selectedRow.iid] : undefined}
      />

      <AddAccountDialog
        open={addOpen}
        counts={platformCounts}
        onClose={() => setAddOpen(false)}
        onScanned={afterAddScanned}
      />

      <SwitchDialog
        target={switchTarget}
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
