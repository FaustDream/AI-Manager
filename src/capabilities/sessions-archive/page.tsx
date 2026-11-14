// 已归档会话（独立页）：从会话页拆出来的「历史仓」，入口在 设置 → 已归档会话。
// 选择交互与活跃页一致：Ctrl/Cmd + 点击行多选，「全选」按筛选选中全部（不只是当前页）。
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useRef, useState, type MouseEvent } from 'react';
import { Button } from '@/components/ui/button';
import { Checkbox, Select, SkeletonRows } from '@/components/ui/form';
import { Notice, Tag } from '@/components/ui';
import { DataTable } from '@/components/ui/table';
import { toast } from '@/components/ui/toast';
import { usePersistedChoice, usePersistedSet } from '@/lib/ui-state';
import type { Filter, Preview, SessionRow } from '@/lib/types';
import { groupArchived, type ArchiveGroup } from '../sessions/grouping';
import { ARCHIVED_COLUMNS, PAGE_SIZE, rowKey } from '../sessions/model';
import { CopyAllButton, PreviewBody } from '../sessions/preview';

const ARCHIVE_GROUPS: readonly ArchiveGroup[] = ['none', 'source', 'project', 'account'];

/** 归档列表的固定筛选：整页只含已归档会话。 */
const ARCHIVE_FILTER: Filter = {
  source: null, account: null, state: 'archived', q: null, origin: null, unnamed: false, sort: 'last_at',
};

export default function ArchivedSessionsPage() {
  const qc = useQueryClient();
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [group, setGroup] = usePersistedChoice<ArchiveGroup>(
    'sessions.archive.group',
    'source',
    ARCHIVE_GROUPS,
  );
  const [collapsed, setCollapsed] = usePersistedSet('sessions.archive.groups');
  const [selected, setSelected] = useState<string | null>(null);

  const list = useInfiniteQuery({
    queryKey: ['sessions', 'archive', 'page'],
    queryFn: ({ pageParam }) =>
      queryCmd('session_page', { cursor: pageParam, limit: PAGE_SIZE, filter: ARCHIVE_FILTER }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
  const items = list.data?.pages.flatMap((page) => page.items) ?? [];
  const total = list.data?.pages[0]?.total ?? 0;

  const preview = useQuery<Preview>({
    queryKey: ['preview', selected],
    queryFn: () => queryCmd('session_preview', { key: selected! }),
    enabled: !!selected,
  });

  const { busy, run: runBusy } = useBusy();

  /** 行选择：切换单行 */
  const toggle = (key: string) => {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  /** 分组折叠：与行选择分开两个集合，避免互相污染 */
  const toggleGroup = (key: string) => {
    const next = new Set(collapsed);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    setCollapsed(next);
  };

  // 行点击：Ctrl / Cmd + 点击 = 切换选中（多选），普通点击 = 打开预览。
  // 两种交互共用一个选择集：全选后再 Ctrl 点掉某行就是「减选」。
  const rowClick = (row: SessionRow, event: MouseEvent<HTMLTableRowElement>) => {
    if (event.ctrlKey || event.metaKey) {
      toggle(rowKey(row));
      return;
    }
    setSelected(rowKey(row));
  };

  const pageKeys = items.map(rowKey);
  const allOnPage = pageKeys.length > 0 && pageKeys.every((key) => checked.has(key));
  const someOnPage = !allOnPage && pageKeys.some((key) => checked.has(key));
  const headRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
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

  /** 一键全选：按归档筛选选中全部（不只是已加载的这页）。 */
  const selectAll = async () => {
    const keys = await runCmd('session_keys', { filter: ARCHIVE_FILTER });
    if (keys === undefined) return;
    setChecked(new Set(keys));
    toast(`已选中 ${keys.length} 条已归档会话`);
  };

  const checkedRows = items.filter((row) => checked.has(rowKey(row)));
  const hasSel = checked.size > 0;

  const unarchive = () =>
    runBusy('unarchive', async () => {
      const keys = checkedRows.map(rowKey);
      if (keys.length === 0) return;
      const count = await runCmd('session_archive', { keys, on: false });
      if (count === undefined) return;
      toast(`已取消归档 ${count} 条会话，已回到活跃列表`);
      setChecked(new Set());
      setSelected(null);
      qc.invalidateQueries({ queryKey: ['facets'] });
      qc.invalidateQueries({ queryKey: ['sessions'] });
    });

  const purge = () =>
    runBusy('purge-archived', async () => {
      const keys = checkedRows.map(rowKey);
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
      setChecked(new Set());
      setSelected(null);
    });

  const groups = groupArchived(items, group);

  return (
    <div className="flex h-[calc(100vh-118px)] flex-col gap-3 overflow-auto">
      <section className="flex min-h-0 flex-col gap-2">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">已归档会话</h2>
          <Tag tone="warn">已归档</Tag>
          <span className="text-xs text-muted">
            共 {total} 条 · 归档只写本地标记，取消归档即回到活跃列表
          </span>
        </div>

        <div className="flex flex-wrap items-center gap-2 text-sm">
          <label className="flex items-center gap-1 text-muted">
            分组
            <Select value={group} onChange={(e) => setGroup(e.target.value as ArchiveGroup)}>
              <option value="none">不分组</option>
              <option value="source">按软件</option>
              <option value="project">按项目</option>
              <option value="account">按账号</option>
            </Select>
          </label>
          <span className="flex-1" />
          {checked.size > 0 && <span className="text-xs text-muted">已选 {checked.size} 条</span>}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void selectAll()}
            disabled={total === 0 || !!busy}
            title="按归档筛选选中全部会话（不只是当前页）"
          >
            全选{total > 0 ? `（${total}）` : ''}
          </Button>
          {checked.size > 0 && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setChecked(new Set());
                setSelected(null);
              }}
            >
              取消选择
            </Button>
          )}
          <span className="text-xs text-muted">Ctrl / Cmd + 点击行可多选</span>
          <Button
            size="sm"
            disabled={!hasSel || !!busy}
            loading={busy === 'unarchive'}
            onClick={unarchive}
            title="取消归档后回到活跃列表"
          >
            取消归档
          </Button>
          <Button
            size="sm"
            variant="danger"
            disabled={!hasSel || !!busy}
            loading={busy === 'purge-archived'}
            onClick={purge}
            title="从客户端真正删除，不可恢复"
          >
            彻底删除
          </Button>
        </div>

        <div className="min-h-[160px] flex-1 overflow-auto">
          {list.isLoading ? (
            // 骨架行必须待在 <tbody> 里：直接塞进 div 会被浏览器搬出表格
            <table className="w-full table-fixed border-collapse">
              <tbody>
                <SkeletonRows rows={8} cols={ARCHIVED_COLUMNS.length + 1} />
              </tbody>
            </table>
          ) : items.length === 0 ? (
            <div className="py-6 text-center text-sm text-muted">还没有已归档的会话。</div>
          ) : group === 'none' ? (
            <DataTable<SessionRow>
              className="w-full table-fixed border-collapse"
              stickyHead
              columns={ARCHIVED_COLUMNS}
              rows={items}
              rowKey={rowKey}
              empty={null}
              lead={{
                width: 32,
                head: (
                  <input
                    ref={headRef}
                    type="checkbox"
                    aria-label="选中本页全部归档"
                    className="accent-[var(--color-accent)]"
                    checked={allOnPage}
                    onChange={togglePage}
                  />
                ),
                cell: (row) => (
                  <span onClick={(e) => e.stopPropagation()}>
                    <Checkbox
                      checked={checked.has(rowKey(row))}
                      onChange={() => toggle(rowKey(row))}
                    />
                  </span>
                ),
              }}
              onRowClick={rowClick}
              rowClassName={(row) =>
                selected === rowKey(row) ? 'cursor-pointer bg-hover' : 'cursor-pointer hover:bg-hover'
              }
            />
          ) : (
            groups.map((g) => (
              <div key={g.key} className="mb-2">
                <button
                  type="button"
                  className="flex w-full cursor-pointer items-center gap-2 border-b border-line bg-hover px-2 py-1 text-left text-xs font-semibold"
                  onClick={() => toggleGroup(g.key)}
                >
                  <span>{collapsed.has(g.key) ? '▸' : '▾'}</span>
                  <span>{g.label}</span>
                  <span className="text-muted">{g.rows.length} 条</span>
                </button>
                {!collapsed.has(g.key) && (
                  <DataTable<SessionRow>
                    dense
                    columns={ARCHIVED_COLUMNS.filter(
                      (c) =>
                        !(
                          (group === 'source' && c.key === 'source') ||
                          (group === 'project' && c.key === 'project') ||
                          (group === 'account' && c.key === 'account')
                        ),
                    )}
                    rows={g.rows}
                    rowKey={rowKey}
                    empty={null}
                    lead={{
                      width: 32,
                      cell: (row) => (
                        <span onClick={(e) => e.stopPropagation()}>
                          <Checkbox
                            checked={checked.has(rowKey(row))}
                            onChange={() => toggle(rowKey(row))}
                          />
                        </span>
                      ),
                    }}
                    onRowClick={rowClick}
                    rowClassName={(row) =>
                      selected === rowKey(row) ? 'cursor-pointer bg-hover' : 'cursor-pointer hover:bg-hover'
                    }
                  />
                )}
              </div>
            ))
          )}
          {list.hasNextPage && (
            <div className="p-2 text-center">
              <Button
                size="sm"
                loading={list.isFetchingNextPage}
                onClick={() => void list.fetchNextPage()}
              >
                加载更多
              </Button>
            </div>
          )}
          {!list.hasNextPage && items.length > 0 && (
            <div className="p-2 text-center text-xs text-muted">共 {items.length} 条，到底了</div>
          )}
        </div>
      </section>

      {/* 正文预览：与活跃页同一套渲染（sessions/preview.tsx）；没选中就不占位 */}
      {selected && (
        <div className="max-h-[40%] flex-none overflow-auto rounded-lg border border-line bg-card p-3">
          {preview.isLoading && (
            <div className="space-y-2 py-3">
              {[0, 1, 2].map((i) => (
                <div key={i} className="h-4 animate-pulse rounded bg-hover" />
              ))}
            </div>
          )}
          {preview.data && (
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
      )}
    </div>
  );
}
