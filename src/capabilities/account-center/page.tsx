// 账号中心：九列列定义驱动 + 拖动排序 + 列宽持久化 + 详情面板。
// 行 key = iid（四处共用的主键）；界面任何位置不渲染 uid（契约 17）。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useRef, useState } from 'react';
import { call, errText } from '@/lib/ipc';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import type { AccountView, ScanReport, TaskView } from '@/lib/types';
import { COLUMNS, type Cell } from './columns';
import { DetailPanel } from './detail';
import { Tag } from '@/components/ui';

const PLATFORMS_FOR_ADD = ['qoder', 'qoder_intl', 'workbuddy', 'workbuddy_intl', 'zcode'];
const ADD_LABELS: Record<string, string> = {
  qoder: 'Qoder CN',
  qoder_intl: 'Qoder 国际版',
  workbuddy: 'WorkBuddy 国内版',
  workbuddy_intl: 'WorkBuddy 国际版',
  zcode: 'ZCode',
};

function CellView({ cell }: { cell: Cell }) {
  return (
    <span
      title={cell.title}
      className={
        cell.tone === 'ok' ? 'text-ok' : cell.tone === 'warn' ? 'text-warn' : cell.tone === 'bad' ? 'text-bad' : cell.tone === 'muted' ? 'text-muted' : ''
      }
    >
      {cell.text}
    </span>
  );
}

export default function AccountCenterPage() {
  const qc = useQueryClient();
  const accounts = useQuery<AccountView[]>({
    queryKey: ['accounts'],
    queryFn: () => call<AccountView[]>('account_list'),
  });
  const current = useQuery<string[]>({
    queryKey: ['current'],
    queryFn: () => call<string[]>('account_current'),
  });
  const tasks = useQuery<TaskView[]>({
    queryKey: ['tasks'],
    queryFn: () => call<TaskView[]>('task_list'),
    refetchInterval: 2000,
  });

  const [selected, setSelected] = useState<string | null>(null);
  const [widths, setWidths] = useState<Record<string, number>>({});
  const [addOpen, setAddOpen] = useState(false);
  const rows = accounts.data ?? [];
  const selectedRow = rows.find((r) => r.iid === selected) ?? null;

  const batchTask = (tasks.data ?? []).find(
    (t) => t.name.startsWith('批量签到') && (t.state === 'running' || t.state === 'queued' || t.state === 'cancelling'),
  );

  // ---- 列宽：持久化在 kv ui.columns（v1 同键） ----
  useEffect(() => {
    call<Record<string, number> | null>('settings_get', { key: 'ui.columns' })
      .then((saved) => saved && setWidths(saved))
      .catch(() => {});
  }, []);
  const saveWidths = (next: Record<string, number>) => {
    setWidths(next);
    call('settings_set', { key: 'ui.columns', value: next }).catch(() => {});
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

  // ---- 拖动排序（落点 = 移除源行后的插入位置；持久化 account_reorder）----
  const [dragging, setDragging] = useState<string | null>(null);
  const [dropAt, setDropAt] = useState<{ iid: string; before: boolean } | null>(null);
  const commitReorder = async () => {
    if (!dragging || !dropAt || dragging === dropAt.iid) return;
    const ids = rows.map((r) => r.iid).filter((id) => id !== dragging);
    const at = ids.indexOf(dropAt.iid);
    ids.splice(dropAt.before ? at : at + 1, 0, dragging);
    try {
      await call('account_reorder', { iids: ids });
      qc.invalidateQueries({ queryKey: ['accounts'] });
    } catch (e) {
      toast(errText(e));
    }
    setDragging(null);
    setDropAt(null);
  };

  const widthOf = (key: string, fallback: number) => widths[key] ?? fallback;

  // 慢操作的按钮加载态：扫描/刷积分要等几秒，不给 busy 态看起来就像没点上
  const [busy, setBusy] = useState<string | null>(null);
  const runBusy = async (key: string, work: () => Promise<void>) => {
    if (busy) return;
    setBusy(key);
    try {
      await work();
    } finally {
      setBusy(null);
    }
  };

  const scanAndRefresh = () =>
    runBusy('scan', async () => {
      try {
        const report = await call<ScanReport>('account_scan');
        toast(`扫描完成：${report.scanned} 个账号（新增 ${report.added}）`);
        qc.invalidateQueries({ queryKey: ['accounts'] });
      } catch (e) {
        toast(errText(e));
      }
    });

  const refreshCredits = () =>
    runBusy('credits', async () => {
      const iids = rows.filter((r) => r.has_secret).map((r) => r.iid);
      try {
        await call('checkin_credits', { iids });
        toast(`已刷新 ${iids.length} 个账号的积分`);
        qc.invalidateQueries({ queryKey: ['accounts'] });
      } catch (e) {
        toast(errText(e));
      }
    });

  const batchCheckin = async () => {
    const iids = rows
      .filter((r) => r.requires_checkin && r.has_secret && r.checked_in !== true)
      .map((r) => r.iid);
    if (iids.length === 0) {
      toast('没有需要签到的账号');
      return;
    }
    try {
      await call<number>('checkin_batch', { iids });
      toast(`批量签到已开始（${iids.length} 个账号）`);
    } catch (e) {
      toast(errText(e));
    }
  };

  const prepareAdd = async (platform: string) => {
    setAddOpen(false);
    try {
      const msg = await call<string>('account_add_prepare', { platform });
      toast(msg);
    } catch (e) {
      toast(errText(e));
    }
  };

  const table = useMemo(() => COLUMNS, []);

  return (
    <div className="flex flex-col gap-2.5" style={{ overflowX: 'auto' }}>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={scanAndRefresh} disabled={!!busy}>
          {busy === 'scan' ? '扫描中…' : '扫描客户端'}
        </Button>
        <Button onClick={() => setAddOpen(true)} disabled={!!busy}>
          添加账号
        </Button>
        <Button onClick={refreshCredits} disabled={!!busy}>
          {busy === 'credits' ? '刷新中…' : '刷新积分'}
        </Button>
        <Button onClick={batchCheckin} disabled={!!batchTask}>
          批量签到
        </Button>
        {batchTask && (
          <span className="flex items-center gap-2 text-xs text-muted">
            批量签到中…
            <span className="inline-block h-1.5 w-24 overflow-hidden rounded-full bg-hover">
              <span
                className="block h-full bg-accent transition-all"
                style={{ width: `${Math.round((batchTask.progress ?? 0) * 100)}%` }}
              />
            </span>
          </span>
        )}
      </div>

      <table className="w-full table-fixed border-collapse">
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
            <th className="border-b border-line-strong px-2.5 py-1.5" />
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
            const isCurrent = (current.data ?? []).includes(row.iid);
            const isDragging = dragging === row.iid;
            const dropBefore = dropAt?.iid === row.iid && dropAt.before;
            const dropAfter = dropAt?.iid === row.iid && !dropAt.before;
            return (
              <tr
                key={row.iid}
                draggable
                onDragStart={() => setDragging(row.iid)}
                onDragOver={(e) => {
                  e.preventDefault();
                  const rect = e.currentTarget.getBoundingClientRect();
                  setDropAt({ iid: row.iid, before: e.clientX - rect.left < rect.width / 2 });
                }}
                onDragEnd={commitReorder}
                onClick={() => setSelected(row.iid)}
                className={`row cursor-pointer hover:bg-hover ${selected === row.iid ? 'bg-hover' : ''} ${
                  isDragging ? 'opacity-40' : ''
                }`}
                style={dropBefore ? { boxShadow: 'inset 0 2px 0 var(--color-accent)' } : dropAfter ? { boxShadow: 'inset 0 -2px 0 var(--color-accent)' } : undefined}
              >
                <td className="px-1 py-1.5 text-center text-muted" style={{ cursor: 'grab' }}>
                  ⠿
                </td>
                {table.map((col) => (
                  <td
                    key={col.key}
                    className="overflow-hidden text-ellipsis whitespace-nowrap border-b border-line px-2.5 py-1.5 align-middle"
                  >
                    <CellView cell={col.cell(row)} />
                  </td>
                ))}
                <td className="border-b border-line px-2.5 py-1.5" onClick={(e) => e.stopPropagation()}>
                  <span className="flex items-center gap-1">
                    {isCurrent && <Tag tone="ok">当前</Tag>}
                    <button
                      className="text-xs text-muted hover:text-fg"
                      title="复制 UID"
                      onClick={async () => {
                        await navigator.clipboard.writeText(row.iid).catch(() => {});
                        toast('UID 已复制');
                      }}
                    >
                      复制 UID
                    </button>
                  </span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      <DetailPanel account={selectedRow} />

      <Dialog open={addOpen} title="添加账号" onClose={() => setAddOpen(false)} wide>
        <p className="mb-3 whitespace-pre-wrap text-sm text-muted">
          借客户端完成登录：选平台 → 本工具退出当前登录态（原登录态会备份）→ 自动打开客户端停在登录页
          → 你在客户端里登录新账号 → 回来点「扫描客户端」。凭据不经过任何第三方。
        </p>
        <div className="grid grid-cols-2 gap-2">
          {PLATFORMS_FOR_ADD.map((p) => (
            <Button key={p} className="justify-start" onClick={() => prepareAdd(p)}>
              {ADD_LABELS[p]}
            </Button>
          ))}
        </div>
      </Dialog>
    </div>
  );
}
