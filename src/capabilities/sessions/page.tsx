// 会话页：活跃列表 + 独立归档列表。
//
// 两个列表的边界是硬的：
//   - 活跃列表永远不含已归档（归档只能在下方「已归档会话」里看到）；
//   - 归档列表只含已归档。
// 分组交互也是两套：活跃列表按项目/账号/状态，归档列表按软件/项目/账号。
// 归档只写本地标记；可见会话不能直接删，彻底删除只出现在归档/特殊状态上。
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useRef, useState } from 'react';
import { errText } from '@/lib/ipc';
import { fmtSize, fmtTime } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Checkbox, Select, SkeletonRows } from '@/components/ui/form';
import { Notice, Tag } from '@/components/ui';
import { DataTable, type Column } from '@/components/ui/table';
import { toast } from '@/components/ui/toast';
import type { Facets, Filter, Message, Preview, SessionRow } from '@/lib/types';
import {
  STATE_LABELS,
  groupActive,
  groupArchived,
  type ActiveGroup,
  type ArchiveGroup,
} from './grouping';

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
const STATE_TONES: Record<string, 'ok' | 'warn' | 'bad' | 'muted'> = {
  visible: 'ok', archived: 'warn', deleted: 'bad', cloud: 'muted', old_account: 'muted',
  orphan: 'warn',
};
/** 自动归档档位：关闭 + 四档。0 = 关闭，后端只认这几个值。 */
const RETENTION_CHOICES = [
  { days: 0, label: '关闭' },
  { days: 3, label: '3 天' },
  { days: 7, label: '7 天' },
  { days: 15, label: '15 天' },
  { days: 30, label: '30 天' },
];
/** 活跃列表里可以直接彻底删除的状态（可见会话必须先归档）。
 *  孤儿是父会话已消失的内部会话，客户端里点不到，只能在这里清理。 */
const PURGEABLE = new Set(['deleted', 'cloud', 'old_account', 'orphan']);

const rowKey = (r: SessionRow) => `${r.kind}|${r.source_key}|${r.account}|${r.session_id}`;

export default function SessionsPage() {
  const qc = useQueryClient();
  const [filter, setFilter] = useState<Filter>({
    source: null, account: null, state: null, sort: 'last_at',
  });
  // 两个列表各自选中、各自分组、各自滚动，互不干扰
  const [activeChecked, setActiveChecked] = useState<Set<string>>(new Set());
  const [archivedChecked, setArchivedChecked] = useState<Set<string>>(new Set());
  const [activeGroup, setActiveGroup] = useState<ActiveGroup>('none');
  const [archiveGroup, setArchiveGroup] = useState<ArchiveGroup>('none');
  const [activeCollapsed, setActiveCollapsed] = useState<Set<string>>(new Set());
  const [archiveCollapsed, setArchiveCollapsed] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string | null>(null);

  const facets = useQuery<Facets>({
    queryKey: ['facets'],
    queryFn: () => queryCmd('session_facets'),
  });

  // 活跃列表：默认永远排除已归档（后端 SQL 已按此口径）
  const filterKey = JSON.stringify(filter);
  const activeQuery = useInfiniteQuery({
    queryKey: ['sessions', 'page', filterKey],
    queryFn: ({ pageParam }) =>
      queryCmd('session_page', { cursor: pageParam, limit: PAGE_SIZE, filter }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
  const activeItems = activeQuery.data?.pages.flatMap((page) => page.items) ?? [];

  // 归档列表：独立的筛选（固定 state=archived）与分页游标
  const archiveFilter: Filter = { source: null, account: null, state: 'archived', sort: 'last_at' };
  const archiveQuery = useInfiniteQuery({
    queryKey: ['sessions', 'archive', 'page'],
    queryFn: ({ pageParam }) =>
      queryCmd('session_page', { cursor: pageParam, limit: PAGE_SIZE, filter: archiveFilter }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
  const archivedItems = archiveQuery.data?.pages.flatMap((page) => page.items) ?? [];

  useEffect(() => {
    setActiveChecked(new Set());
    setSelected(null);
  }, [filterKey]);

  const sentinel = useRef<HTMLDivElement | null>(null);
  const { fetchNextPage, hasNextPage, isFetchingNextPage } = activeQuery;
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
        qc.invalidateQueries({ queryKey: ['facets'] });
        qc.invalidateQueries({ queryKey: ['sessions'] });
      })
      .catch((e) => toast(errText(e)));
  };

  const toggle = (set: Set<string>, apply: (next: Set<string>) => void, key: string) => {
    const next = new Set(set);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    apply(next);
  };

  // 活跃列表：可见 → 归档；特殊状态 → 彻底删除
  const activeCheckedRows = activeItems.filter((row) => activeChecked.has(rowKey(row)));
  const activeVisible = activeCheckedRows.filter((row) => row.state === 'visible');
  const activeSpecial = activeCheckedRows.filter((row) => PURGEABLE.has(row.state));
  const activeHasSel = activeChecked.size > 0;

  const pageKeys = activeItems.map(rowKey);
  const allOnPage = pageKeys.length > 0 && pageKeys.every((key) => activeChecked.has(key));
  const someOnPage = !allOnPage && pageKeys.some((key) => activeChecked.has(key));
  const headRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (headRef.current) headRef.current.indeterminate = someOnPage;
  }, [someOnPage]);

  const togglePage = () => {
    setActiveChecked((prev) => {
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
    setActiveChecked(new Set(keys));
    toast(`已选中 ${keys.length} 条`);
  };

  // 异步按钮的加载态（与账号中心同一套 Button loading）
  const { busy, run: runBusy } = useBusy();

  // 归档只写本地标记：不关客户端、不删文件，随时可以取消
  const archiveActive = () =>
    runBusy('archive', async () => {
      const keys = activeVisible.map(rowKey);
      if (keys.length === 0) {
        toast('只有「可见」状态的会话可以归档；特殊状态请用「彻底删除」');
        return;
      }
      const count = await runCmd('session_archive', { keys, on: true });

      if (count === undefined) return;
      toast(`已归档 ${count} 条会话（可在下方「已归档会话」查看）`);
      setActiveChecked(new Set());
      setSelected(null);
      qc.invalidateQueries({ queryKey: ['facets'] });
      qc.invalidateQueries({ queryKey: ['sessions'] });
    });

  const purgeActive = () =>
    runBusy('purge', async () => {
      const keys = activeSpecial.map(rowKey);
      if (keys.length === 0) return;
      const clients = await queryCmd('session_running_clients', { keys }).catch(
        () => [] as string[],
      );
      const ok = await confirmAction({
        title: '彻底删除会话',
        message:
          `彻底删除选中的 ${keys.length} 条特殊状态会话（已删除 / 云端 / 旧账号 / 孤立）？\n\n` +
          '会从对应客户端真正删掉数据，不可恢复。' +
          (clients.length ? `\n\n需要先临时关闭：${clients.join('、')}（删完自动重启）` : ''),
        danger: true,
      });
      if (!ok) return;
      if ((await runCmd('session_purge', { keys })) === undefined) return;
      setActiveChecked(new Set());
      setSelected(null);
    });

  // 归档列表：取消归档 / 彻底删除
  const archivedCheckedRows = archivedItems.filter((row) => archivedChecked.has(rowKey(row)));
  const archivedHasSel = archivedChecked.size > 0;

  const unarchive = () =>
    runBusy('unarchive', async () => {
      const keys = archivedCheckedRows.map(rowKey);
      if (keys.length === 0) return;
      const count = await runCmd('session_archive', { keys, on: false });
      if (count === undefined) return;
      toast(`已取消归档 ${count} 条会话，已回到活跃列表`);
      setArchivedChecked(new Set());
      setSelected(null);
      qc.invalidateQueries({ queryKey: ['facets'] });
      qc.invalidateQueries({ queryKey: ['sessions'] });
    });

  const purgeArchived = () =>
    runBusy('purge-archived', async () => {
      const keys = archivedCheckedRows.map(rowKey);
      if (keys.length === 0) return;
      const clients = await queryCmd('session_running_clients', { keys }).catch(
        () => [] as string[],
      );
      const ok = await confirmAction({
        title: '彻底删除已归档会话',
        message:
          `彻底删除选中的 ${keys.length} 条已归档会话？\n\n` +
          '会从对应客户端真正删掉数据，不可恢复。' +
          (clients.length ? `\n\n需要先临时关闭：${clients.join('、')}（删完自动重启）` : ''),
        danger: true,
      });
      if (!ok) return;
      if ((await runCmd('session_purge', { keys })) === undefined) return;
      setArchivedChecked(new Set());
      setSelected(null);
    });

  const scan = () =>
    runBusy('scan', async () => {
      if ((await runCmd('session_scan')) === undefined) return;
      toast('会话扫描已开始（后台任务）');
      setTimeout(() => qc.invalidateQueries({ queryKey: ['facets'] }), 3000);
    });

  const activeTotal = activeQuery.data?.pages[0]?.total ?? 0;
  const archiveTotal = archiveQuery.data?.pages[0]?.total ?? 0;

  const toggleGroup = (set: Set<string>, apply: (next: Set<string>) => void, key: string) =>
    toggle(set, apply, key);

  const activeGroups = groupActive(activeItems, activeGroup);
  const archiveGroups = groupArchived(archivedItems, archiveGroup);

  return (
    <div className="flex h-[calc(100vh-118px)] flex-col gap-3 overflow-auto">
      {/* ---------------------------------------------------------- 活跃会话 */}
      <section className="flex min-h-0 flex-col gap-2">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">活跃会话</h2>
          <span className="text-xs text-muted">共 {activeTotal} 条</span>
        </div>

        {/* 来源标签栏：按软件分栏；删掉的会话不会再出现在对应软件里。 */}
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
              {(facets.data?.states ?? []).map((s) => (
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
          <label className="flex items-center gap-1 text-muted">
            分组
            <Select
              value={activeGroup}
              onChange={(e) => setActiveGroup(e.target.value as ActiveGroup)}
            >
              <option value="none">不分组</option>
              <option value="project">按项目</option>
              <option value="account">按账号</option>
              <option value="state">按状态</option>
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
          <span className="flex-1" />
          <Button size="sm" onClick={scan} loading={busy === 'scan'} disabled={!!busy}>
            重新扫描
          </Button>
        </div>

        {/* 选择操作单独一行：左边说清楚「选了几条」，右边才是动作 */}
        <div className="flex flex-wrap items-center gap-2 text-sm">
          {activeChecked.size > 0 && (
            <span className="text-xs text-muted">已选 {activeChecked.size} 条</span>
          )}
          {allOnPage && activeTotal > activeItems.length && activeChecked.size < activeTotal && (
            <button
              type="button"
              className="cursor-pointer text-xs text-accent hover:underline"
              onClick={() => void selectAllMatching()}
            >
              选中全部 {activeTotal} 条
            </button>
          )}
          <span className="flex-1" />
          <Button
            size="sm"
            disabled={!activeHasSel || activeVisible.length === 0 || !!busy}
            loading={busy === 'archive'}
            onClick={archiveActive}
            title="归档后不再出现在活跃列表；可见会话必须先归档才能彻底删除"
          >
            归档{activeVisible.length > 0 ? `（${activeVisible.length}）` : ''}
          </Button>
          {activeSpecial.length > 0 && (
            <Button
              size="sm"
              variant="danger"
              disabled={!activeHasSel || !!busy}
              loading={busy === 'purge'}
              onClick={purgeActive}
              title="特殊状态（已删除 / 云端 / 旧账号 / 孤立）可直接彻底删除"
            >
              彻底删除（{activeSpecial.length}）
            </Button>
          )}
        </div>

        <div className="min-h-[140px] flex-1 overflow-auto">
          {activeItems.length === 0 ? (
            <div className="py-6 text-center text-sm text-muted">
              {activeQuery.isLoading ? '加载中…' : '没有符合条件的活跃会话。'}
            </div>
          ) : activeGroup === 'none' ? (
            <DataTable<SessionRow>
              className="w-full table-fixed border-collapse"
              stickyHead
              columns={SESSION_COLUMNS}
              rows={activeItems}
              rowKey={rowKey}
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
                    <Checkbox
                      checked={activeChecked.has(rowKey(row))}
                      onChange={() => toggle(activeChecked, setActiveChecked, rowKey(row))}
                    />
                  </span>
                ),
              }}
              onRowClick={(row) => setSelected(rowKey(row))}
              rowClassName={(row) =>
                selected === rowKey(row) ? 'cursor-pointer bg-hover' : 'cursor-pointer hover:bg-hover'
              }
            />
          ) : (
            activeGroups.map((group) => (
              <div key={group.key} className="mb-2">
                <button
                  type="button"
                  className="flex w-full cursor-pointer items-center gap-2 border-b border-line bg-hover px-2 py-1 text-left text-xs font-semibold"
                  onClick={() =>
                    toggleGroup(activeCollapsed, setActiveCollapsed, group.key)
                  }
                >
                  <span>{activeCollapsed.has(group.key) ? '▸' : '▾'}</span>
                  <span>{group.label}</span>
                  <span className="text-muted">{group.rows.length} 条</span>
                </button>
                {!activeCollapsed.has(group.key) && (
                  <DataTable<SessionRow>
                    dense
                    columns={SESSION_COLUMNS.filter(
                      (c) =>
                        !(
                          (activeGroup === 'project' && c.key === 'project') ||
                          (activeGroup === 'account' && c.key === 'account') ||
                          (activeGroup === 'state' && c.key === 'state')
                        ),
                    )}
                    rows={group.rows}
                    rowKey={rowKey}
                    empty={null}
                    lead={{
                      width: 32,
                      cell: (row) => (
                        <span onClick={(e) => e.stopPropagation()}>
                          <Checkbox
                            checked={activeChecked.has(rowKey(row))}
                            onChange={() => toggle(activeChecked, setActiveChecked, rowKey(row))}
                          />
                        </span>
                      ),
                    }}
                    onRowClick={(row) => setSelected(rowKey(row))}
                    rowClassName={(row) =>
                      selected === rowKey(row) ? 'cursor-pointer bg-hover' : 'cursor-pointer hover:bg-hover'
                    }
                  />
                )}
              </div>
            ))
          )}
          <div ref={sentinel} className="h-px" />
          {isFetchingNextPage && <div className="p-2 text-center text-xs text-muted">加载中…</div>}
          {!hasNextPage && activeItems.length > 0 && (
            <div className="p-2 text-center text-xs text-muted">共 {activeItems.length} 条，到底了</div>
          )}
        </div>
      </section>

      {/* ---------------------------------------------------------- 归档列表 */}
      <section className="flex min-h-0 flex-col gap-2 rounded-lg border-t-2 border-line-strong bg-card/60 p-3">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">已归档会话</h2>
          <Tag tone="warn">已归档</Tag>
          <span className="text-xs text-muted">共 {archiveTotal} 条</span>
        </div>

        <div className="flex flex-wrap items-center gap-2 text-sm">
          <label className="flex items-center gap-1 text-muted">
            分组
            <Select
              value={archiveGroup}
              onChange={(e) => setArchiveGroup(e.target.value as ArchiveGroup)}
            >
              <option value="none">不分组</option>
              <option value="source">按软件</option>
              <option value="project">按项目</option>
              <option value="account">按账号</option>
            </Select>
          </label>
          <span className="flex-1" />
          {archivedChecked.size > 0 && (
            <span className="text-xs text-muted">已选 {archivedChecked.size} 条</span>
          )}
          <Button
            size="sm"
            disabled={!archivedHasSel || !!busy}
            loading={busy === 'unarchive'}
            onClick={unarchive}
            title="取消归档后回到活跃列表"
          >
            取消归档
          </Button>
          <Button
            size="sm"
            variant="danger"
            disabled={!archivedHasSel || !!busy}
            loading={busy === 'purge-archived'}
            onClick={purgeArchived}
            title="从客户端真正删除，不可恢复"
          >
            彻底删除
          </Button>
        </div>

        <div className="min-h-[120px] flex-1 overflow-auto">
          {archivedItems.length === 0 ? (
            <div className="py-6 text-center text-sm text-muted">
              {archiveQuery.isLoading ? '加载中…' : '还没有已归档的会话。'}
            </div>
          ) : archiveGroup === 'none' ? (
            <DataTable<SessionRow>
              className="w-full table-fixed border-collapse"
              stickyHead
              columns={SESSION_COLUMNS}
              rows={archivedItems}
              rowKey={rowKey}
              empty={null}
              lead={{
                width: 32,
                head: (
                  <input
                    type="checkbox"
                    aria-label="选中本页全部归档"
                    className="accent-[var(--color-accent)]"
                    checked={
                      archivedItems.length > 0 &&
                      archivedItems.every((row) => archivedChecked.has(rowKey(row)))
                    }
                    onChange={() => {
                      const all = archivedItems.every((row) => archivedChecked.has(rowKey(row)));
                      const next = new Set(archivedChecked);
                      for (const row of archivedItems) {
                        if (all) next.delete(rowKey(row));
                        else next.add(rowKey(row));
                      }
                      setArchivedChecked(next);
                    }}
                  />
                ),
                cell: (row) => (
                  <span onClick={(e) => e.stopPropagation()}>
                    <Checkbox
                      checked={archivedChecked.has(rowKey(row))}
                      onChange={() => toggle(archivedChecked, setArchivedChecked, rowKey(row))}
                    />
                  </span>
                ),
              }}
              onRowClick={(row) => setSelected(rowKey(row))}
              rowClassName={(row) =>
                selected === rowKey(row) ? 'cursor-pointer bg-hover' : 'cursor-pointer hover:bg-hover'
              }
            />
          ) : (
            archiveGroups.map((group) => (
              <div key={group.key} className="mb-2">
                <button
                  type="button"
                  className="flex w-full cursor-pointer items-center gap-2 border-b border-line bg-hover px-2 py-1 text-left text-xs font-semibold"
                  onClick={() =>
                    toggleGroup(archiveCollapsed, setArchiveCollapsed, group.key)
                  }
                >
                  <span>{archiveCollapsed.has(group.key) ? '▸' : '▾'}</span>
                  <span>{group.label}</span>
                  <span className="text-muted">{group.rows.length} 条</span>
                </button>
                {!archiveCollapsed.has(group.key) && (
                  <DataTable<SessionRow>
                    dense
                    columns={SESSION_COLUMNS.filter(
                      (c) =>
                        !(
                          (archiveGroup === 'source' && c.key === 'source') ||
                          (archiveGroup === 'project' && c.key === 'project') ||
                          (archiveGroup === 'account' && c.key === 'account')
                        ),
                    )}
                    rows={group.rows}
                    rowKey={rowKey}
                    empty={null}
                    lead={{
                      width: 32,
                      cell: (row) => (
                        <span onClick={(e) => e.stopPropagation()}>
                          <Checkbox
                            checked={archivedChecked.has(rowKey(row))}
                            onChange={() => toggle(archivedChecked, setArchivedChecked, rowKey(row))}
                          />
                        </span>
                      ),
                    }}
                    onRowClick={(row) => setSelected(rowKey(row))}
                    rowClassName={(row) =>
                      selected === rowKey(row) ? 'cursor-pointer bg-hover' : 'cursor-pointer hover:bg-hover'
                    }
                  />
                )}
              </div>
            ))
          )}
          {archiveQuery.hasNextPage && (
            <div className="p-2 text-center">
              <Button
                size="sm"
                loading={archiveQuery.isFetchingNextPage}
                onClick={() => void archiveQuery.fetchNextPage()}
              >
                加载更多
              </Button>
            </div>
          )}
          {!archiveQuery.hasNextPage && archivedItems.length > 0 && (
            <div className="p-2 text-center text-xs text-muted">
              共 {archivedItems.length} 条，到底了
            </div>
          )}
        </div>
      </section>

      {/* ---------------------------------------------------------- 正文预览 */}
      <div className="max-h-[40%] flex-none overflow-auto rounded-lg border border-line bg-card p-3">
        {!selected && <p className="text-sm text-muted">点一行看正文预览。</p>}
        {selected && preview.isLoading && <SkeletonRows rows={3} cols={1} />}
        {selected && preview.data && (
          <>
            <div className="mb-2 flex items-center gap-2">
              <h3 className="min-w-0 flex-1 truncate text-sm font-semibold">
                {[...activeItems, ...archivedItems].find((r) => rowKey(r) === selected)?.title ||
                  '（无标题）'}
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
