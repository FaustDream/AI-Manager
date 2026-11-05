// 账号中心：八列列定义驱动 + 拖动排序 + 列宽持久化 + 操作列 + 详情面板。
// 行 key = iid（四处共用的主键）；界面任何位置不渲染 uid（契约 17）。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useMemo, useRef, useState } from 'react';
import { errKind, errText } from '@/lib/ipc';
import { SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import type { AccountView } from '@/lib/types';
import { COLUMNS, type Cell } from './columns';
import { DetailPanel } from './detail';
import { OPS_WIDTH, RestoreDialog, RowActions, SwitchDialog } from './actions';
import { AddAccountDialog, AccountToolbar } from './toolbar';
import { useRowDrag } from './use-row-drag';

function CellView({ cell }: { cell: Cell }) {
  return (
    <span
      title={cell.title}
      className={
        cell.tone === 'ok' ? 'text-ok' : cell.tone === 'warn' ? 'text-warn' : cell.tone === 'bad' ? 'text-bad' : cell.tone === 'muted' ? 'text-muted' : ''
      }
    >
      <span className="block truncate leading-tight">{cell.text}</span>
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
  const rows = accounts.data ?? [];
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

  // 慢操作的按钮加载态：扫描/刷积分/切换/恢复要等几秒，不给 busy 态看起来就像没点上
  const { busy, run: runBusy } = useBusy();
  // 聚焦时自动同步要读「当前有没有任务在跑」：effect 里拿不到最新的 state，用 ref 过渡
  const busyRef = useRef<string | null>(null);
  busyRef.current = busy;

  // ---- 拖动排序：指针逻辑在 use-row-drag，这里只负责持久化 ----
  const { dragging, dropAt, startDrag } = useRowDrag(rows, busy, async (iids) => {
    
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

  const scanAndRefresh = () =>
    runBusy('scan', async () => {
      
        const report = await runCmd('account_scan');

        if (report === undefined) return;
        toast(`扫描完成：${report.scanned} 个账号（新增 ${report.added}）`);
        qc.invalidateQueries({ queryKey: ['accounts'] });
      
    });

  // 切回窗口时静默同步一次：登录/换号多半是在客户端里做的，回来列表应该已经是最新的。
  // 不弹 toast——用户没点任何按钮，不该被打断；5 秒内重复聚焦不重扫。
  const lastScanAt = useRef(0);
  useEffect(() => {
    const onFocus = () => {
      const now = Date.now();
      if (busyRef.current) return;
      if (now - lastScanAt.current < 5000) return;
      lastScanAt.current = now;
      void queryCmd('account_scan')
        .then(() => {
          qc.invalidateQueries({ queryKey: ['accounts'] });
          qc.invalidateQueries({ queryKey: ['current'] });
        })
        .catch(() => {});
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [qc]);

  const refreshCredits = () =>
    runBusy('credits', async () => {
      const iids = rows.filter((r) => r.has_secret).map((r) => r.iid);
      if (iids.length === 0) {
        toast('没有可刷新的账号（先扫描客户端）');
        return;
      }
      
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
        const failed = Object.keys(errors).length;
        toast(`已刷新 ${iids.length} 个账号的积分${failed ? `（${failed} 个查询失败）` : ''}`);
        qc.invalidateQueries({ queryKey: ['accounts'] });
        qc.invalidateQueries({ queryKey: ['detail'] });
      
    });

  const batchCheckin = async () => {
    const iids = rows
      .filter((r) => r.requires_checkin && r.has_secret && r.checked_in !== true)
      .map((r) => r.iid);
    if (iids.length === 0) {
      toast('没有需要签到的账号');
      return;
    }
    
      if ((await runCmd('checkin_batch', { iids })) === undefined) return;
      toast(`批量签到已开始（${iids.length} 个账号）`);
    
  };

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

  const removeAccount = async (row: AccountView) => {
    // 不可逆动作（同页的会话删除、清空回收站都有确认）
    if (!(await confirmAction({ title: '确认操作', message: `从账号库移除「${row.name}」？\n只删本机记录，客户端登录态不动。`, danger: true }))) return;
    
      if ((await runCmd('account_delete', { iid: row.iid })) === undefined) return;
      toast('已从账号库移除（客户端登录态未动）');
      if (selected === row.iid) setSelected(null);
      qc.invalidateQueries({ queryKey: ['accounts'] });
    
  };

  /** 列表里不渲染 uid（契约 17），只有点这一下才按需取一次。 */
  const copyUid = async (row: AccountView) => {
    
      const uid = await runCmd('account_uid', { iid: row.iid });

      if (uid === undefined) return;
      await navigator.clipboard.writeText(uid);
      toast('UID 已复制');
    
  };

  const table = useMemo(() => COLUMNS, []);

  return (
    <div className="flex flex-col gap-2.5">
      <AccountToolbar
        busy={busy}
        onScan={scanAndRefresh}
        onAdd={() => setAddOpen(true)}
        onRefresh={refreshCredits}
        onCheckin={batchCheckin}
      />

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
                  onMouseDown={(e) => {
                    resizing.current = {
                      key: col.key,
                      startX: e.clientX,
                      startW: widthOf(col.key, col.width),
                    };
                    document.body.classList.add('resizing');
                  }}
                />
              </th>
            ))}
            <th
              className={`border-b border-line-strong px-2.5 py-1.5 text-right text-xs font-semibold text-muted ${
                narrow ? 'sticky right-0 z-10 border-l border-line-strong bg-bg' : ''
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
          {!accounts.isLoading && rows.length === 0 && (
            <tr>
              <td colSpan={COLUMNS.length + 2}>
                <StateBlock kind="empty" title="还没有账号" detail="点「扫描客户端」把各平台已登录的账号收进来。" />
              </td>
            </tr>
          )}
          {rows.map((row) => {
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
                  style={{ cursor: 'grab', touchAction: 'none' }}
                  onPointerDown={(e) => startDrag(e, row.iid)}
                  title="拖动排序"
                >
                  ⠿
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
                      ? `sticky right-0 z-10 border-l border-line ${
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
                    onCopyUid={() => void copyUid(row)}
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
