// 会话页：活跃列表 + 正文预览。已归档的会话在独立的「已归档会话」页
// （入口在 设置 → 已归档会话，见 capabilities/sessions-archive）。
//
// 活跃列表永远不含已归档（后端 SQL 已按此口径）；归档只写本地标记。
// 可见会话不能直接删，彻底删除只出现在特殊状态上。
// 多选两种交互：Ctrl / Cmd + 点击行切换单行；「全选」按当前筛选选中全部（不只是当前页）。
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useMemo, useRef, useState, type MouseEvent } from 'react';
import { errText } from '@/lib/ipc';
import { Button } from '@/components/ui/button';
import { Checkbox, Select, SkeletonRows } from '@/components/ui/form';
import { Notice } from '@/components/ui';
import { DataTable } from '@/components/ui/table';
import { toast } from '@/components/ui/toast';
import { usePersistedChoice, usePersistedSet } from '@/lib/ui-state';
import type { Facets, Filter, Preview, SessionRow } from '@/lib/types';
import {
  currentScope,
  groupActive,
  isPluginFacet,
  isPluginKey,
  scopeSources,
  type ActiveGroup,
} from './grouping';
import { PAGE_SIZE, SESSION_COLUMNS, rowKey } from './model';
import { CopyAllButton, PreviewBody } from './preview';

const ACTIVE_GROUPS: readonly ActiveGroup[] = [
  'none', 'source', 'project', 'account', 'state',
];

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

export default function SessionsPage() {
  const qc = useQueryClient();
  const [filter, setFilter] = useState<Filter>({
    source: null, account: null, state: null, q: null, origin: null, unnamed: false, sort: 'last_at',
  });
  // 活跃列表的选中与分组；分组选择跨重启保持
  const [activeChecked, setActiveChecked] = useState<Set<string>>(new Set());
  const [activeGroup, setActiveGroup] = usePersistedChoice<ActiveGroup>(
    'sessions.active.group',
    'source',
    ACTIVE_GROUPS,
  );
  // 折叠状态落 localStorage：刷新页面、切走再回来都保持原样
  const [activeCollapsed, setActiveCollapsed] = usePersistedSet('sessions.active.groups');
  const [selected, setSelected] = useState<string | null>(null);

  const facets = useQuery<Facets>({
    queryKey: ['facets'],
    queryFn: () => queryCmd('session_facets'),
  });

  // 「来源」下拉的选项范围只跟着标签走（currentScope）：选了插件源也留在 IDEA 范围，
  // 聚合项不消失、同组其他源也能直接切；其他软件范围只出它自己。
  const scopedSources = useMemo(
    () => scopeSources(facets.data?.sources ?? [], currentScope(filter)),
    [facets.data, filter],
  );

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

  // 行点击：Ctrl / Cmd + 点击 = 切换该行选中（多选），普通点击 = 打开预览。
  // 两种交互共用一个选择集：全选后再 Ctrl 点掉某行就是「减选」。
  const rowClick = (row: SessionRow, event: MouseEvent<HTMLTableRowElement>) => {
    if (event.ctrlKey || event.metaKey) {
      toggle(activeChecked, setActiveChecked, rowKey(row));
      return;
    }
    setSelected(rowKey(row));
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

  /** 一键全选：按当前筛选选中全部（不只是已加载的这页）。 */
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
      toast(`已归档 ${count} 条会话（在 设置 → 已归档会话 里查看）`);
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

  const scan = () =>
    runBusy('scan', async () => {
      const report = await runCmd('session_scan');
      if (report === undefined) return;
      // 来源错误由全局事件桥统一提示（并做去重），这里只报扫描结果
      toast(`扫描完成：写入 ${report.rows} 条`);
      qc.invalidateQueries({ queryKey: ['facets'] });
      qc.invalidateQueries({ queryKey: ['sessions'] });
    });

  const activeTotal = activeQuery.data?.pages[0]?.total ?? 0;

  const toggleGroup = (set: Set<string>, apply: (next: Set<string>) => void, key: string) =>
    toggle(set, apply, key);

  const activeGroups = groupActive(activeItems, activeGroup);

  return (
    <div className="flex h-[calc(100vh-118px)] flex-col gap-3 overflow-auto">
      {/* ---------------------------------------------------------- 活跃会话 */}
      <section className="flex min-h-0 flex-col gap-2">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold">活跃会话</h2>
          <span className="text-xs text-muted">共 {activeTotal} 条</span>
        </div>

        {/* 来源标签栏：插件会话全部收进「IDEA 插件」聚合标签，各软件插件源不再单独成标签。 */}
        <div className="flex flex-wrap items-center gap-1 border-b border-line pb-2">
          <button
            className={`rounded-md px-3 py-1 text-sm ${
              filter.source === null && filter.origin === null
                ? 'bg-accent font-medium text-on-accent'
                : 'text-muted hover:bg-hover hover:text-fg'
            }`}
            onClick={() => setFilter((f) => ({ ...f, source: null, origin: null }))}
          >
            全部
          </button>
          {(facets.data?.sources ?? []).some(isPluginFacet) && (
            <button
              className={`rounded-md px-3 py-1 text-sm ${
                filter.origin === 'ide-plugin' || (filter.origin === null && isPluginKey(filter.source))
                  ? 'bg-accent font-medium text-on-accent'
                  : 'text-muted hover:bg-hover hover:text-fg'
              }`}
              onClick={() => setFilter((f) => ({ ...f, source: null, origin: 'ide-plugin' }))}
              title="聚合 Qoder / Qoder CN / Trae / CodeBuddy 的 IDEA 插件会话；子源可在「来源」下拉里单独筛"
            >
              IDEA 插件
            </button>
          )}
          {(facets.data?.sources ?? []).filter((s) => !isPluginFacet(s)).map((s) => (
            <button
              key={s.value}
              className={`rounded-md px-3 py-1 text-sm ${
                filter.source === s.value && filter.origin === null
                  ? 'bg-accent font-medium text-on-accent'
                  : 'text-muted hover:bg-hover hover:text-fg'
              }`}
              onClick={() => setFilter((f) => ({ ...f, source: s.value, origin: null }))}
            >
              {s.label}
            </button>
          ))}
        </div>

        <div className="flex flex-wrap items-center gap-3 text-sm">
          {/* 来源筛选：选项范围只跟着标签走；IDEA 标签本身就是聚合视图，下拉只留各插件源的细筛。 */}
          <label className="flex items-center gap-1 text-muted">
            来源
            <Select
              value={filter.source ?? ''}
              onChange={(e) => {
                const value = e.target.value;
                if (value === '') {
                  // 「全部」= 回到当前标签的默认视图：全部标签清筛选；
                  // IDEA 标签回到聚合；单源标签留在它自己（结果集相同）。
                  const scope = currentScope(filter);
                  setFilter((f) => ({
                    ...f,
                    source: scope === 'all' || scope === 'ide-plugin' ? null : scope,
                    origin: scope === 'ide-plugin' ? 'ide-plugin' : null,
                  }));
                } else {
                  setFilter((f) => ({ ...f, source: value, origin: null }));
                }
              }}
            >
              <option value="">全部</option>
              {scopedSources.map((s) => (
                <option key={s.value} value={s.value}>
                  {s.label}（{s.count}）
                </option>
              ))}
            </Select>
          </label>
          {/* 未命名快筛：与来源 / 状态 / 搜索叠加使用；角标数与活跃列表同口径（不含已归档） */}
          <button
            className={`rounded-md px-2 py-1 text-sm ${
              filter.unnamed
                ? 'bg-accent font-medium text-on-accent'
                : 'border border-line text-muted hover:bg-hover hover:text-fg'
            }`}
            onClick={() => setFilter((f) => ({ ...f, unnamed: !f.unnamed }))}
            title="只看标题是「（未命名会话）」占位文案的会话，可与来源 / 状态 / 搜索叠加"
          >
            未命名{facets.data?.unnamed ? `（${facets.data.unnamed}）` : ''}
          </button>
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
            搜索
            <input
              className="w-48 rounded-md border border-line bg-transparent px-2 py-1 text-sm"
              value={filter.q ?? ''}
              onChange={(e) => setFilter((f) => ({ ...f, q: e.target.value || null }))}
              placeholder="标题 / 项目 / 来源 / 插件"
            />
          </label>
          <label className="flex items-center gap-1 text-muted">
            分组
            <Select
              value={activeGroup}
              onChange={(e) => setActiveGroup(e.target.value as ActiveGroup)}
            >
              <option value="none">不分组</option>
              <option value="source">按软件</option>
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
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void selectAllMatching()}
            disabled={activeTotal === 0 || !!busy}
            title="按当前筛选选中全部会话（不只是当前页）"
          >
            全选{activeTotal > 0 ? `（${activeTotal}）` : ''}
          </Button>
          {activeChecked.size > 0 && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setActiveChecked(new Set());
                setSelected(null);
              }}
            >
              取消选择
            </Button>
          )}
          <span className="text-xs text-muted">Ctrl / Cmd + 点击行可多选</span>
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
          {activeQuery.isLoading ? (
            // 加载态画成表格骨架：这一页的主体就是列表，直接铺出来比一句「加载中…」更像话
            // 骨架行必须待在 <tbody> 里：直接塞进 div 会被浏览器搬出表格
            <table className="w-full table-fixed border-collapse">
              <tbody>
                <SkeletonRows rows={8} cols={SESSION_COLUMNS.length + 1} />
              </tbody>
            </table>
          ) : activeItems.length === 0 ? (
            <div className="py-6 text-center text-sm text-muted">没有符合条件的活跃会话。</div>
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
              onRowClick={rowClick}
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
                    onRowClick={rowClick}
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

      {/* ---------------------------------------------------------- 正文预览 */}
      {/* 没选中行就不占位：空盒子加一句「点一行看正文预览」只是噪音，列表还能多两行 */}
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
                  {activeItems.find((r) => rowKey(r) === selected)?.title || '（无标题）'}
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
