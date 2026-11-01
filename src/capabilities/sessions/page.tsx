// 会话页：筛选三下拉 + 排序（时间/大小/消息数）+ 游标分页无限滚动 + 正文预览 +
// 删除（进回收站）+ 备份 + 回收站管理。行 key = kind|source|account|session_id。
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { call, errText } from '@/lib/ipc';
import { fmtSize, fmtTime } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, Select, SkeletonRows } from '@/components/ui/form';
import { Notice, Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import type {
  DeleteReport, Facets, Filter, Manifest, Message, Page, Preview, RestoreReport, SessionRow, Snapshot,
} from '@/lib/types';

const PAGE_SIZE = 100;
const STATE_LABELS: Record<string, string> = {
  visible: '可见', deleted: '已删除', old_account: '旧账号', cloud: '云端', archived: '已归档',
};

export default function SessionsPage() {
  const qc = useQueryClient();
  const [filter, setFilter] = useState<Filter>({ source: null, account: null, state: null, sort: 'last_at' });
  const [selected, setSelected] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [trashOpen, setTrashOpen] = useState(false);
  const [backupsOpen, setBackupsOpen] = useState(false);

  const facets = useQuery<Facets>({
    queryKey: ['facets'],
    queryFn: () => call<Facets>('session_facets'),
  });

  // 游标分页交给 useInfiniteQuery：queryKey 带上 filterKey，换筛选天然开新请求，
  // 旧响应不会覆盖新列表（手写 useState 累加在这里有竞态）；key 挂在 ['sessions']
  // 前缀下，事件桥「扫描完成」的失效能直接命中本页
  const filterKey = JSON.stringify(filter);
  const listQuery = useInfiniteQuery({
    queryKey: ['sessions', 'page', filterKey],
    queryFn: ({ pageParam }) =>
      call<Page<SessionRow>>('session_page', { cursor: pageParam, limit: PAGE_SIZE, filter }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
  const items = listQuery.data?.pages.flatMap((page) => page.items) ?? [];

  useEffect(() => {
    setChecked(new Set());
    setSelected(null);
  }, [filterKey]);

  const sentinel = useRef<HTMLDivElement | null>(null);
  const { fetchNextPage, hasNextPage, isFetchingNextPage } = listQuery;
  useEffect(() => {
    const node = sentinel.current;
    if (!node) return;
    const io = new IntersectionObserver((entries) => {
      if (entries[0].isIntersecting && hasNextPage && !isFetchingNextPage) {
        void fetchNextPage();
      }
    });
    io.observe(node);
    return () => io.disconnect();
  }, [fetchNextPage, hasNextPage, isFetchingNextPage]);

  const preview = useQuery<Preview>({
    queryKey: ['preview', selected],
    queryFn: () => call<Preview>('session_preview', { key: selected }),
    enabled: !!selected,
  });

  const toggle = (key: string) => {
    setChecked((old) => {
      const next = new Set(old);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const rowKey = (r: SessionRow) => `${r.kind}|${r.source_key}|${r.account}|${r.session_id}`;

  // 异步按钮的加载态（与账号中心同一套 Button loading）
  const [busy, setBusy] = useState<string | null>(null);
  const runBusy = (key: string, work: () => Promise<void>) => {
    if (busy) return;
    setBusy(key);
    void work().finally(() => setBusy(null));
  };

  const deleteChecked = () =>
    runBusy('delete', async () => {
      const keys = [...checked];
      if (keys.length === 0) return;
      if (!window.confirm(`删除选中的 ${keys.length} 条会话？（进回收站，可恢复）`)) return;
      try {
        const report = await call<DeleteReport>('session_delete', { keys });
        toast(
          `已删除 ${report.sessions} 条（${report.files} 个文件）进回收站` +
            (report.errors.length ? `；失败 ${report.errors.length}` : ''),
        );
        setChecked(new Set());
        setSelected(null);
        qc.invalidateQueries({ queryKey: ['facets'] });
        qc.invalidateQueries({ queryKey: ['sessions'] });
      } catch (e) {
        toast(errText(e));
      }
    });

  const scan = () =>
    runBusy('scan', async () => {
      try {
        await call('session_scan');
        toast('会话扫描已开始（后台任务）');
        // 扫描是后台任务：起个轮询等它收尾，完成时列表自然被事件桥刷新
        setTimeout(() => qc.invalidateQueries({ queryKey: ['facets'] }), 3000);
      } catch (e) {
        toast(errText(e));
      }
    });

  const backup = () =>
    runBusy('backup', async () => {
      try {
        const snap = await call<Snapshot>('session_backup');
        toast(`快照 ${snap.tag}：${snap.sources.length} 个来源，${fmtSize(snap.bytes)}`);
        qc.invalidateQueries({ queryKey: ['backups'] });
      } catch (e) {
        toast(errText(e));
      }
    });

  const total = facets.data?.total ?? 0;
  return (
    <div className="flex h-[calc(100vh-118px)] flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <label className="flex items-center gap-1 text-muted">
          来源
          <Select
            value={filter.source ?? ''}
            onChange={(e) => setFilter((f) => ({ ...f, source: e.target.value || null }))}
          >
            <option value="">全部</option>
            {(facets.data?.sources ?? []).map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}（{s.count}）
              </option>
            ))}
          </Select>
        </label>
        <label className="flex items-center gap-1 text-muted">
          账号
          <Select
            value={filter.account ?? ''}
            onChange={(e) => setFilter((f) => ({ ...f, account: e.target.value || null }))}
          >
            <option value="">全部</option>
            {(facets.data?.accounts ?? []).map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}（{s.count}）
              </option>
            ))}
          </Select>
        </label>
        <label className="flex items-center gap-1 text-muted">
          状态
          <Select
            value={filter.state ?? ''}
            onChange={(e) => setFilter((f) => ({ ...f, state: e.target.value || null }))}
          >
            <option value="">全部</option>
            {(facets.data?.states ?? []).map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}（{s.count}）
              </option>
            ))}
          </Select>
        </label>
        <label className="flex items-center gap-1 text-muted">
          排序
          <Select
            value={filter.sort ?? 'last_at'}
            onChange={(e) => setFilter((f) => ({ ...f, sort: e.target.value as Filter['sort'] }))}
          >
            <option value="last_at">最近时间</option>
            <option value="size">占用大小</option>
            <option value="msgs">消息数</option>
          </Select>
        </label>
        <span className="flex-1" />
        <Button size="sm" onClick={scan} loading={busy === 'scan'} disabled={!!busy}>
          重新扫描
        </Button>
        <Button size="sm" onClick={backup} loading={busy === 'backup'} disabled={!!busy}>
          整库备份
        </Button>
        <Button size="sm" onClick={() => setBackupsOpen(true)}>备份列表</Button>
        <Button size="sm" onClick={() => setTrashOpen(true)}>回收站</Button>
        <Button
          size="sm"
          variant="danger"
          disabled={checked.size === 0 || !!busy}
          loading={busy === 'delete'}
          onClick={deleteChecked}
        >
          删除选中（{checked.size}）
        </Button>
      </div>

      <div className="min-h-[180px] flex-1 overflow-auto">
        <table className="w-full table-fixed border-collapse">
          <thead className="sticky top-0 bg-bg">
            <tr>
              <th className="w-8 border-b border-line-strong px-1 py-1.5" />
              <th className="border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted">标题</th>
              <th className="w-36 border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted">来源</th>
              <th className="w-32 border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted">账号</th>
              <th className="w-28 border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted">时间</th>
              <th className="w-20 border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted">消息</th>
              <th className="w-24 border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted">大小</th>
              <th className="w-20 border-b border-line-strong px-2.5 py-1.5 text-left text-xs font-semibold text-muted">状态</th>
            </tr>
          </thead>
          <tbody>
            {items.length === 0 && (listQuery.isLoading || total === 0) && (
              <SkeletonRows rows={6} cols={8} />
            )}
            {items.map((row) => {
              const key = rowKey(row);
              return (
                <tr
                  key={key}
                  className="cursor-pointer hover:bg-hover"
                  style={selected === key ? { background: 'var(--color-hover)' } : undefined}
                  onClick={() => setSelected(key)}
                >
                  <td className="px-1 py-1.5 text-center" onClick={(e) => e.stopPropagation()}>
                    <Checkbox
                      checked={checked.has(key)}
                      onChange={() => toggle(key)}
                      disabled={row.state !== 'visible'}
                    />
                  </td>
                  <td className="overflow-hidden text-ellipsis whitespace-nowrap border-b border-line px-2.5 py-1.5">
                    {row.title || '（无标题）'}
                  </td>
                  <td className="overflow-hidden text-ellipsis whitespace-nowrap border-b border-line px-2.5 py-1.5 text-muted">
                    {row.source_label}
                  </td>
                  <td className="overflow-hidden text-ellipsis whitespace-nowrap border-b border-line px-2.5 py-1.5 text-muted">
                    {row.account_label}
                  </td>
                  <td className="border-b border-line px-2.5 py-1.5 tabular-nums text-muted">{fmtTime(row.last_at)}</td>
                  <td className="border-b border-line px-2.5 py-1.5 tabular-nums text-muted">{row.msgs ?? '—'}</td>
                  <td className="border-b border-line px-2.5 py-1.5 tabular-nums text-muted">{fmtSize(row.size)}</td>
                  <td className="border-b border-line px-2.5 py-1.5">
                    <Tag tone={row.state === 'visible' ? 'ok' : 'muted'}>
                      {STATE_LABELS[row.state] ?? row.state}
                    </Tag>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
        <div ref={sentinel} className="h-px" />
        {isFetchingNextPage && <div className="p-2 text-center text-xs text-muted">加载中…</div>}
        {!hasNextPage && items.length > 0 && (
          <div className="p-2 text-center text-xs text-muted">共 {items.length} 条，到底了</div>
        )}
      </div>

      <div className="max-h-[46%] flex-none overflow-auto rounded-lg border border-line bg-card p-3">
        {!selected && <p className="text-sm text-muted">点一行看正文预览。</p>}
        {selected && preview.isLoading && <SkeletonRows rows={3} cols={1} />}
        {selected && preview.data && (
          <>
            <div className="mb-2 flex items-center gap-2">
              <h3 className="min-w-0 flex-1 truncate text-sm font-semibold">
                {items.find((r) => rowKey(r) === selected)?.title || '（无标题）'}
              </h3>
              <CopyAllButton messages={preview.data.messages} />
            </div>
            {preview.data.truncated && <Notice className="mb-2">正文太长，只显示头尾部分（共 {preview.data.total} 条，略去 {preview.data.dropped} 条）。</Notice>}
            {preview.data.note && <Notice tone="warn" className="mb-2">{preview.data.note}</Notice>}
            <div className="mt-2 border-t border-dashed border-line pt-2">
              <PreviewBody messages={preview.data.messages} />
            </div>
          </>
        )}
      </div>

      <TrashDrawer open={trashOpen} onClose={() => setTrashOpen(false)} />
      <BackupsDrawer open={backupsOpen} onClose={() => setBackupsOpen(false)} />
    </div>
  );
}

// ------------------------------------------------------------------ 长文本渲染
// 长会话一次挂几百条 DOM 会卡（v1 的教训：列表行不做逐行动画同理），
// 所以两层节流：外层按条数渐进渲染，内层超长单条折叠。

const PREVIEW_CHUNK = 50;
const FOLD_LIMIT = 3000;
const FOLD_KEEP = 2000;

/** 超长单条：默认只显示前 FOLD_KEEP 字，展开/收起由用户点。 */
function Foldable({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  if (text.length <= FOLD_LIMIT) {
    return <div className="whitespace-pre-wrap break-words">{text}</div>;
  }
  return (
    <div className="whitespace-pre-wrap break-words">
      {open ? text : `${text.slice(0, FOLD_KEEP)}\n…（余 ${text.length - FOLD_KEEP} 字）`}
      <button
        className="ml-2 text-xs text-accent hover:underline"
        onClick={() => setOpen(!open)}
      >
        {open ? '收起' : '展开'}
      </button>
    </div>
  );
}

/** 渐进渲染：先挂 PREVIEW_CHUNK 条，「展开」按批追加。切换会话时重置。 */
function PreviewBody({ messages }: { messages: Message[] }) {
  const [shown, setShown] = useState(PREVIEW_CHUNK);
  useEffect(() => setShown(PREVIEW_CHUNK), [messages]);
  if (messages.length === 0) {
    return <p className="text-sm text-muted">没有可读的消息。</p>;
  }
  return (
    <>
      {messages.slice(0, shown).map((msg, i) => (
        <div
          key={i}
          className={`mb-2 border-l-2 pl-2 text-sm ${
            msg.role === 'user' ? 'border-l-accent' : 'border-l-ok'
          }`}
        >
          <div className="text-[11px] tracking-wide text-muted">{msg.role}</div>
          <Foldable text={msg.text} />
        </div>
      ))}
      {shown < messages.length && (
        <button
          className="mt-1 rounded-md border border-line px-3 py-1 text-xs hover:bg-hover"
          onClick={() => setShown((n) => n + PREVIEW_CHUNK)}
        >
          展开剩余 {messages.length - shown} 条
        </button>
      )}
    </>
  );
}

/** 复制全文：折叠与渐进都不影响复制——复制的是全量消息。 */
function CopyAllButton({ messages }: { messages: Message[] }) {
  const [done, setDone] = useState(false);
  return (
    <Button
      size="sm"
      variant="ghost"
      disabled={messages.length === 0}
      onClick={async () => {
        const text = messages.map((m) => `${m.role}: ${m.text}`).join('\n\n');
        await navigator.clipboard.writeText(text).catch(() => {});
        setDone(true);
        toast(`已复制 ${messages.length} 条消息到剪贴板`);
        setTimeout(() => setDone(false), 2000);
      }}
    >
      {done ? '已复制' : '复制全文'}
    </Button>
  );
}

function TrashDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const batches = useQuery<Manifest[]>({
    queryKey: ['trash'],
    queryFn: () => call<Manifest[]>('session_trash'),
    enabled: open,
  });

  const [busyTag, setBusyTag] = useState<string | null>(null);

  const restore = (tag: string) => {
    if (busyTag) return;
    setBusyTag(tag);
    void call<RestoreReport>('session_trash_restore', { tag })
      .then((r) => {
        toast(`已恢复 ${r.restored} 条${r.errors.length ? `；失败 ${r.errors.length}` : ''}`);
        // 恢复会改动回收站批次与主列表（行会重新出现），两者都要失效
        qc.invalidateQueries({ queryKey: ['trash'] });
        qc.invalidateQueries({ queryKey: ['facets'] });
        qc.invalidateQueries({ queryKey: ['sessions'] });
      })
      .catch((e) => toast(errText(e)))
      .finally(() => setBusyTag(null));
  };

  const purge = (tag: string | null) => {
    if (busyTag) return;
    if (!window.confirm(tag ? `彻底删除批次 ${tag}？不可恢复。` : '清空整个回收站？不可恢复。')) return;
    setBusyTag(tag ?? '__all__');
    void call<number>('session_trash_purge', { tag })
      .then((bytes) => {
        toast(`已释放 ${fmtSize(bytes)}`);
        qc.invalidateQueries({ queryKey: ['trash'] });
      })
      .catch((e) => toast(errText(e)))
      .finally(() => setBusyTag(null));
  };

  return (
    <Dialog open={open} title="会话回收站" onClose={onClose} wide>
      {batches.isLoading && <SkeletonRows rows={3} cols={2} />}
      {(batches.data ?? []).length === 0 && !batches.isLoading && (
        <p className="text-sm text-muted">回收站是空的。</p>
      )}
      <div className="flex flex-col gap-2">
        {(batches.data ?? []).map((batch) => (
          <div key={batch.tag} className="rounded-md border border-line p-2 text-sm">
            <div className="flex items-center gap-2">
              <span className="font-mono text-xs">{batch.tag}</span>
              <span className="text-xs text-muted">{fmtTime(batch.at)} · {batch.items.length} 个文件</span>
              <span className="flex-1" />
              <Button
                size="sm"
                onClick={() => restore(batch.tag)}
                loading={busyTag === batch.tag}
                disabled={!!busyTag}
              >
                恢复
              </Button>
              <Button
                size="sm"
                variant="danger"
                onClick={() => purge(batch.tag)}
                loading={busyTag === batch.tag}
                disabled={!!busyTag}
              >
                彻底删除
              </Button>
            </div>
            {batch.platforms.length > 0 && (
              <div className="mt-1 text-xs text-muted">涉及客户端：{batch.platforms.join('、')}</div>
            )}
          </div>
        ))}
      </div>
      {(batches.data ?? []).length > 0 && (
        <div className="mt-3 flex justify-end">
          <Button
            variant="danger"
            onClick={() => purge(null)}
            loading={busyTag === '__all__'}
            disabled={!!busyTag}
          >
            清空回收站
          </Button>
        </div>
      )}
    </Dialog>
  );
}

function BackupsDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const backups = useQuery<Snapshot[]>({
    queryKey: ['backups'],
    queryFn: () => call<Snapshot[]>('session_backups'),
    enabled: open,
  });
  return (
    <Dialog open={open} title="整库备份（保留最近 10 份）" onClose={onClose} wide>
      {(backups.data ?? []).length === 0 && <p className="text-sm text-muted">还没有备份。</p>}
      <div className="flex flex-col gap-2">
        {(backups.data ?? []).map((snap) => (
          <div key={snap.tag} className="rounded-md border border-line p-2 text-sm">
            <div className="flex items-center gap-2">
              <span className="font-mono text-xs">{snap.tag}</span>
              <span className="text-xs text-muted">{fmtTime(snap.at)}</span>
              <span className="flex-1" />
              <span className="text-xs tabular-nums">{fmtSize(snap.bytes)}</span>
            </div>
            <div className="mt-1 text-xs text-muted">
              来源：{snap.sources.join('、')}
              {snap.skipped.length > 0 && `；跳过（太大）：${snap.skipped.join('、')}`}
            </div>
          </div>
        ))}
      </div>
    </Dialog>
  );
}
