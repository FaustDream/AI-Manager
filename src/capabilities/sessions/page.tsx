// 会话页：状态/排序下拉 + 自动归档档位 + 游标分页无限滚动 + 正文预览 +
// 删除（进回收站）+ 备份 + 回收站管理。行 key = kind|source|account|session_id。
// 工具栏只保留不重复的三组：来源标签 / 筛选 / 动作；归档相关的入口只留一处。
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { BackupsDrawer, TrashDrawer } from './dialogs';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { findActiveTask, useTasks } from '@/lib/use-tasks';
import { useEffect, useMemo, useRef, useState } from 'react';
import { errText } from '@/lib/ipc';
import { fmtSize, fmtTime } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Checkbox, Select, SkeletonRows } from '@/components/ui/form';
import { Notice, Tag } from '@/components/ui';
import { DataTable, type Column } from '@/components/ui/table';
import { toast } from '@/components/ui/toast';
import type { Facets, Filter, Message, Preview, SessionRow } from '@/lib/types';

const PAGE_SIZE = 100;

/** 主列表的列定义（C6）：加一列 = 加一行。 */
const SESSION_COLUMNS: Column<SessionRow>[] = [
  {
    key: 'title',
    header: '标题',
    truncate: true,
    cell: (row) => row.title || '（无标题）',
  },
  {
    key: 'project',
    header: '项目',
    width: 144,
    truncate: true,
    className: 'text-muted',
    title: (row) => row.cwd || undefined,
    cell: (row) => row.project || '—',
  },
  { key: 'source', header: '来源', width: 144, truncate: true, className: 'text-muted', cell: (row) => row.source_label },
  { key: 'account', header: '账号', width: 128, truncate: true, className: 'text-muted', cell: (row) => row.account_label },
  { key: 'time', header: '时间', width: 112, className: 'tabular-nums text-muted', cell: (row) => fmtTime(row.last_at) },
  { key: 'msgs', header: '消息', width: 80, className: 'tabular-nums text-muted', cell: (row) => row.msgs ?? '—' },
  { key: 'size', header: '大小', width: 96, className: 'tabular-nums text-muted', cell: (row) => fmtSize(row.size) },
  {
    key: 'state',
    header: '状态',
    width: 80,
    cell: (row) => (
      <Tag tone={STATE_TONES[row.state] ?? 'muted'}>{STATE_LABELS[row.state] ?? row.state}</Tag>
    ),
  },
];
const STATE_LABELS: Record<string, string> = {
  visible: '可见', deleted: '已删除', old_account: '旧账号', cloud: '云端', archived: '已归档',
};
const STATE_TONES: Record<string, 'ok' | 'warn' | 'bad' | 'muted'> = {
  visible: 'ok', archived: 'warn', deleted: 'bad', cloud: 'muted', old_account: 'muted',
};
/** 自动归档档位：关闭 + 四档。0 = 关闭，后端只认这几个值。 */
const RETENTION_CHOICES = [
  { days: 0, label: '关闭' },
  { days: 3, label: '3 天' },
  { days: 7, label: '7 天' },
  { days: 15, label: '15 天' },
  { days: 30, label: '30 天' },
];
/** 可以「彻底删除」的状态：归档过，或客户端那边已经删了。 */
const PURGEABLE = new Set(['archived', 'deleted']);

export default function SessionsPage() {
  const qc = useQueryClient();
  const [filter, setFilter] = useState<Filter>({
    source: null, account: null, state: null, sort: 'last_at', show_archived: false,
  });
  const [selected, setSelected] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [trashOpen, setTrashOpen] = useState(false);
  const [backupsOpen, setBackupsOpen] = useState(false);

  // 任务表：删除/彻底删除都在后台跑，这里取「本页正在跑的任务」画进度条
  const tasks = useTasks({ refetchInterval: 1000 });
  const job = findActiveTask(tasks.data, 'sessions');

  const facets = useQuery<Facets>({
    queryKey: ['facets'],
    queryFn: () => queryCmd('session_facets'),
  });
  // 状态下拉 = facets 的状态 + 恒定的「已归档」。facets 里没有已归档时（一条都没归档过）
  // 这一项就消失了，「只看已归档」的视图也点不到——所以这里补回来。
  const stateOptions = useMemo(() => {
    const list = facets.data?.states ?? [];
    if (list.some((s) => s.value === 'archived')) return list;
    return [...list, { value: 'archived', label: '已归档', count: 0 }];
  }, [facets.data]);

  // 游标分页交给 useInfiniteQuery：queryKey 带上 filterKey，换筛选天然开新请求，
  // 旧响应不会覆盖新列表（手写 useState 累加在这里有竞态）；key 挂在 ['sessions']
  // 前缀下，事件桥「扫描完成」的失效能直接命中本页
  const filterKey = JSON.stringify(filter);
  const listQuery = useInfiniteQuery({
    queryKey: ['sessions', 'page', filterKey],
    queryFn: ({ pageParam }) =>
      queryCmd('session_page', { cursor: pageParam, limit: PAGE_SIZE, filter }),
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
    queryFn: () => queryCmd('session_preview', { key: selected! }),
    enabled: !!selected,
  });

  // 自动归档档位存在后端 kv 里：换机器/重启都跟着走，不落 localStorage
  const retention = useQuery<number>({
    queryKey: ['retention'],
    queryFn: () => queryCmd('session_retention'),
  });
  const setRetention = (days: number) => {
    void queryCmd('session_retention_set', { days })
      .then((saved) => {
        qc.setQueryData(['retention'], saved);
        toast(saved === 0 ? '已关闭自动归档' : `超过 ${saved} 天没动的会话会自动归档`);
        // 改档位可能立刻归档一批：列表与计数都要重拉
        qc.invalidateQueries({ queryKey: ['facets'] });
        qc.invalidateQueries({ queryKey: ['sessions'] });
      })
      .catch((e) => toast(errText(e)));
  };

  const toggle = (key: string) => {
    setChecked((old) => {
      const next = new Set(old);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const rowKey = (r: SessionRow) => `${r.kind}|${r.source_key}|${r.account}|${r.session_id}`;

  // 选中行能做什么，全由状态决定（与后端放行规则一一对应）：
  //   归档 / 取消归档 —— 除了「客户端已删除」，谁都能归档
  //   删除（进回收站）—— 未归档、未删除的（含云端）
  //   彻底删除       —— 必须**整批**都是已归档 / 已删除，否则先归档
  // 归档与删除各只留一个按钮：当前选中状态决定它显示成哪个动作，
  // 避免同一条工具栏上摆四个按钮、还各有各的禁用条件。
  // 计数用 checked.size（跨页全选也算），动作依据只看当前已加载的行。
  const checkedRows = items.filter((row) => checked.has(rowKey(row)));
  const hasSel = checked.size > 0;
  const canArchive = checkedRows.some((row) => row.state !== 'archived' && row.state !== 'deleted');
  const canUnarchive = checkedRows.some((row) => row.state === 'archived');
  const canDelete = checkedRows.some((row) => row.state !== 'archived' && row.state !== 'deleted');
  const canPurge = checkedRows.length > 0 && checkedRows.every((row) => PURGEABLE.has(row.state));
  // 归档按钮三态：全是已归档 → 取消归档；能归档 → 归档；只剩已删除 → 不给动作
  const archiveMode: 'archive' | 'unarchive' | 'none' = !hasSel
    ? 'archive'
    : checkedRows.length === 0
      ? 'archive'
      : checkedRows.every((row) => row.state === 'archived')
        ? 'unarchive'
        : canArchive
          ? 'archive'
          : canUnarchive
            ? 'unarchive'
            : 'none';
  // 删除按钮两态：整批都已归档/已删除 → 彻底删除，否则进回收站
  const deleteMode: 'trash' | 'purge' | 'none' = !hasSel
    ? 'trash'
    : checkedRows.length === 0
      ? 'trash'
      : canPurge
        ? 'purge'
        : canDelete
          ? 'trash'
          : 'none';

  // 批量选择：表头勾选框管「本页」，本页全选后再给一个「选中全部 N 条」（跨页）
  const pageKeys = items.map(rowKey);
  const allOnPage = pageKeys.length > 0 && pageKeys.every((key) => checked.has(key));
  const someOnPage = !allOnPage && pageKeys.some((key) => checked.has(key));
  const headRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    // indeterminate 只能走 DOM 属性，React 没有对应 prop
    if (headRef.current) headRef.current.indeterminate = someOnPage;
  }, [someOnPage]);

  const togglePage = () => {
    setChecked((prev) => {
      const next = new Set(prev);
      for (const key of pageKeys) {
        if (allOnPage) next.delete(key);
        else next.add(key);
      }
      return next;
    });
  };

  const selectAllMatching = async () => {
    
      const keys = await runCmd('session_keys', { filter });

      if (keys === undefined) return;
      setChecked(new Set(keys));
      toast(`已选中 ${keys.length} 条`);
    
  };

  // 异步按钮的加载态（与账号中心同一套 Button loading）
  const { busy, run: runBusy } = useBusy();

  const deleteChecked = () =>
    runBusy('delete', async () => {
      const keys = [...checked];
      if (keys.length === 0) return;
      const clients = await queryCmd('session_running_clients', { keys }).catch(
        () => [] as string[],
      );
      const ok = await confirmAction({
        title: '删除会话',
        message:
          `删除选中的 ${keys.length} 条会话？（进回收站，可恢复）` +
          (clients.length ? `\n\n需要先临时关闭：${clients.join('、')}（删完自动重启）` : ''),
        danger: true,
      });
      if (!ok) return;
      // 后端在任务里跑：立刻返回任务 id，进度由任务条展示，结果走 session:delete 事件
      if ((await runCmd('session_delete', { keys })) === undefined) return;
      setChecked(new Set());
      setSelected(null);
    });

  // 真实删除：只放行「已归档 / 已删除」的行，后端会再校验一次（界面置灰只是提示）。
  // 后端顺手把回收站批次销毁，所以这里不用再调 purge。
  const purgeChecked = () =>
    runBusy('purge', async () => {
      const keys = [...checked];
      if (keys.length === 0) return;
      const clients = await queryCmd('session_running_clients', { keys }).catch(
        () => [] as string[],
      );
      const ok = await confirmAction({
        title: '彻底删除会话',
        message:
          `彻底删除选中的 ${keys.length} 条会话？\n\n` +
          '会从对应客户端真正删掉数据，不进回收站，不可恢复。' +
          (clients.length ? `\n\n需要先临时关闭：${clients.join('、')}（删完自动重启）` : ''),
        danger: true,
      });
      if (!ok) return;
      
        if ((await runCmd('session_purge', { keys })) === undefined) return;
        setChecked(new Set());
        setSelected(null);
      
    });

  // 归档只写本地标记：不关客户端、不进回收站，随时可以取消
  const archiveChecked = (on: boolean) =>
    runBusy(on ? 'archive' : 'unarchive', async () => {
      const keys = [...checked];
      if (keys.length === 0) return;
      
        const count = await runCmd('session_archive', { keys, on });

        if (count === undefined) return;
        toast(on ? `已归档 ${count} 条会话` : `已取消归档 ${count} 条会话`);
        setChecked(new Set());
        setSelected(null);
        qc.invalidateQueries({ queryKey: ['facets'] });
        qc.invalidateQueries({ queryKey: ['sessions'] });
      
    });

  const scan = () =>
    runBusy('scan', async () => {
      
        if ((await runCmd('session_scan')) === undefined) return;
        toast('会话扫描已开始（后台任务）');
        // 扫描是后台任务：起个轮询等它收尾，完成时列表自然被事件桥刷新
        setTimeout(() => qc.invalidateQueries({ queryKey: ['facets'] }), 3000);
      
    });

  // 用列表查询返回的 total（已按筛选口径算过），facets 的 total 是全库、不带筛选
  const total = listQuery.data?.pages[0]?.total ?? 0;
  return (
    <div className="flex h-[calc(100vh-118px)] flex-col gap-2.5">
      {/* 来源标签栏：按软件分栏；删掉的会话不会再出现在对应软件里。
          点来源只改来源，不再动状态——状态由下面的下拉管，两边互不干扰。 */}
      <div className="flex flex-wrap items-center gap-1 border-b border-line pb-2">
        <button
          className={`rounded-md px-3 py-1 text-sm ${
            filter.source === null
              ? 'bg-accent font-medium text-on-accent'
              : 'text-muted hover:bg-hover hover:text-fg'
          }`}
          onClick={() => setFilter((f) => ({ ...f, source: null }))}
        >
          全部
        </button>
        {(facets.data?.sources ?? []).map((s) => (
          <button
            key={s.value}
            className={`rounded-md px-3 py-1 text-sm ${
              filter.source === s.value
                ? 'bg-accent font-medium text-on-accent'
                : 'text-muted hover:bg-hover hover:text-fg'
            }`}
            onClick={() => setFilter((f) => ({ ...f, source: s.value }))}
          >
            {s.label}
          </button>
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <label className="flex items-center gap-1 text-muted">
          状态
          <Select
            value={filter.state ?? ''}
            onChange={(e) => setFilter((f) => ({ ...f, state: e.target.value || null }))}
          >
            <option value="">全部</option>
            {/* 已归档不跟着 facets 走：一条都没归档时列表里就没有这一项，
                但「只看已归档」这个视图本身应该一直能点到。 */}
            {stateOptions.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
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
        <label
          className="flex items-center gap-1 text-muted"
          title="超过这个天数没活动的会话，扫描时自动归档"
        >
          自动归档
          <Select
            value={String(retention.data ?? 0)}
            onChange={(e) => setRetention(Number(e.target.value))}
          >
            {RETENTION_CHOICES.map((choice) => (
              <option key={choice.days} value={choice.days}>
                {choice.label}
              </option>
            ))}
          </Select>
        </label>
        <label className="flex items-center gap-1 text-muted">
          <Checkbox
            checked={filter.show_archived}
            onChange={(e) => setFilter((f) => ({ ...f, show_archived: e.target.checked }))}
          />
          显示已归档
        </label>
        <span className="flex-1" />
        <Button size="sm" onClick={scan} loading={busy === 'scan'} disabled={!!busy}>
          重新扫描
        </Button>
        <Button size="sm" onClick={() => setBackupsOpen(true)}>
          备份
        </Button>
        <Button size="sm" onClick={() => setTrashOpen(true)}>
          回收站
        </Button>
      </div>

      {/* 选择操作单独一行：左边说清楚「选了几条」，右边才是动作，避免和筛选混在一起。
          归档与删除各只留一个按钮——当前选中状态决定它显示成哪个动作。 */}
      <div className="flex flex-wrap items-center gap-2 text-sm">
        {checked.size > 0 && <span className="text-xs text-muted">已选 {checked.size} 条</span>}
        {job && (
          <span className="flex items-center gap-2 text-xs text-muted">
            {job.name}
            {/* 有进度才画条：扫描这类任务不上报进度，画个 0% 的空条反而像卡住 */}
            {job.progress != null && (
              <span className="inline-block h-1.5 w-24 overflow-hidden rounded-full bg-hover">
                <span
                  className="block h-full bg-accent transition-[width] duration-300"
                  style={{ width: `${Math.round(job.progress * 100)}%` }}
                />
              </span>
            )}
            <span className="tabular-nums">{job.message || '进行中…'}</span>
          </span>
        )}
        {allOnPage && total > items.length && checked.size < total && (
          <button
            type="button"
            className="cursor-pointer text-xs text-accent hover:underline"
            onClick={() => void selectAllMatching()}
          >
            选中全部 {total} 条
          </button>
        )}
        <span className="flex-1" />
        {archiveMode !== 'none' && (
          <Button
            size="sm"
            disabled={!hasSel || !!busy}
            loading={busy === 'archive' || busy === 'unarchive'}
            onClick={() => archiveChecked(archiveMode === 'archive')}
            title={
              archiveMode === 'archive'
                ? '归档后不再出现在默认列表；归档是彻底删除的前提'
                : '取消归档，重新回到默认列表'
            }
          >
            {archiveMode === 'archive' ? '归档' : '取消归档'}
            {hasSel ? `（${checked.size}）` : ''}
          </Button>
        )}
        {deleteMode !== 'none' && (
          <Button
            size="sm"
            variant="danger"
            disabled={!hasSel || !!busy}
            loading={busy === 'delete' || busy === 'purge'}
            onClick={deleteMode === 'purge' ? purgeChecked : deleteChecked}
            title={
              deleteMode === 'purge'
                ? '从客户端真正删除，不进回收站'
                : '进回收站，可恢复'
            }
          >
            {deleteMode === 'purge' ? '彻底删除' : '删除'}
            {hasSel ? `（${checked.size}）` : ''}
          </Button>
        )}
      </div>

      <div className="min-h-[180px] flex-1 overflow-auto">
        <DataTable<SessionRow>
          className="w-full table-fixed border-collapse"
          stickyHead
          columns={SESSION_COLUMNS}
          rows={items}
          rowKey={rowKey}
          loading={items.length === 0 && (listQuery.isLoading || total === 0)}
          skeletonRows={6}
          empty={null}
          lead={{
            width: 32,
            head: (
              <input
                ref={headRef}
                type="checkbox"
                aria-label="选中本页全部"
                className="accent-[var(--color-accent)]"
                checked={allOnPage}
                onChange={togglePage}
              />
            ),
            cell: (row) => (
              <span onClick={(e) => e.stopPropagation()}>
                <Checkbox checked={checked.has(rowKey(row))} onChange={() => toggle(rowKey(row))} />
              </span>
            ),
          }}
          onRowClick={(row) => setSelected(rowKey(row))}
          rowClassName={(row) =>
            selected === rowKey(row) ? 'cursor-pointer bg-hover' : 'cursor-pointer hover:bg-hover'
          }
        />
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
