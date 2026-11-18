// 会话列表的共享主体：活跃页与「已归档会话」页唯一的实现。
// 之前两页各写一份，筛选/分页/多选慢慢漂移（归档页连搜索都没有）；
// 现在抽成一个组件，mode 只决定：状态口径、批量按钮集合与几个专属控件。
//
// v3 会话管理（docs/session-management-design.md）：置顶 / 本地改名 / 标签 /
// 正文全文搜索 / 已隐藏视图 / 右键菜单 / 导出 / 未命名治理，全部落在这里。
//
// 列表只有一份：所有平台、所有账号的会话合在一起（不按平台拆标签页，也不显示
// 账号）。要看某一维度的子集，用筛选条的平台 / 状态 / 项目下拉；分组只改摆放。
import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useMemo, useRef, useState, type MouseEvent } from 'react';
import { errText } from '@/lib/ipc';
import { Button } from '@/components/ui/button';
import { Checkbox, Input, Select, SkeletonRows } from '@/components/ui/form';
import { Notice, Tag } from '@/components/ui';
import { DataTable } from '@/components/ui/table';
import { toast } from '@/components/ui/toast';
import { usePersistedChoice, usePersistedSet } from '@/lib/ui-state';
import { onFocusSession, takePreset } from '@/lib/nav';
import type { Facets, Filter, Preview, SessionRow } from '@/lib/types';
import { groupRows, type GroupMode } from './grouping';
import { ARCHIVED_COLUMNS, PAGE_SIZE, SESSION_COLUMNS, rowKey } from './model';
import { CopyAllButton, PreviewBody } from './preview';
import { ContextMenu, type MenuItem } from './actions-menu';
import { RenameDialog, SuggestTitlesDialog, TagsDialog, fetchSuggestions } from './dialogs';

/** 分组档位。归档列表整表都是归档件，按状态分组没有意义，所以少一档。 */
const ACTIVE_GROUPS: readonly GroupMode[] = ['none', 'source', 'project', 'state'];
const ARCHIVE_GROUPS: readonly GroupMode[] = ['none', 'source', 'project'];

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

export type SessionsMode = 'active' | 'archived';

export function SessionListPage({ mode }: { mode: SessionsMode }) {
  const qc = useQueryClient();
  const archived = mode === 'archived';

  const [filter, setFilter] = useState<Filter>(() => ({
    source: null,
    project: null,
    state: archived ? 'archived' : null,
    q: null,
    origin: null,
    unnamed: false,
    pinned: false,
    hidden: false,
    body: null,
    tags: [],
    sort: 'last_at',
    ...takePreset<Partial<Filter>>('sessions'),
    // 账号维度已从会话页撤掉（不再显示账号数据）：账号中心下钻即使带了也不生效
    account: null,
  }));
  // 搜索框的草稿与后端筛选分离：输入防抖 250ms 才落到 filter，避免每键一查
  const [searchDraft, setSearchDraft] = useState(filter.q ?? filter.body ?? '');
  const [bodyMode, setBodyMode] = useState(false);
  // 活跃列表的选中与分组；分组选择跨重启保持
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [group, setGroup] = usePersistedChoice<GroupMode>(
    archived ? 'sessions.archive.group' : 'sessions.active.group',
    'none',
    archived ? ARCHIVE_GROUPS : ACTIVE_GROUPS,
  );
  const [collapsed, setCollapsed] = usePersistedSet(
    archived ? 'sessions.archive.groups' : 'sessions.active.groups',
  );
  const [selected, setSelected] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; row: SessionRow } | null>(null);
  const [renameTarget, setRenameTarget] = useState<SessionRow | null>(null);
  const [tagsTarget, setTagsTarget] = useState<SessionRow | null>(null);
  const [suggestItems, setSuggestItems] = useState<Awaited<ReturnType<typeof fetchSuggestions>>>(null);

  const searchRef = useRef<HTMLInputElement | null>(null);

  const facets = useQuery<Facets>({
    queryKey: ['facets'],
    queryFn: () => queryCmd('session_facets'),
  });

  // 平台 / 项目的可选项要跟当前列表口径走：归档页列的是「只剩归档件」的平台，
  // 活跃页列的是活跃口径（后端分两套统计，前端只挑一套）。
  const platformOptions = (archived ? facets.data?.archived_sources : facets.data?.sources) ?? [];
  const projectOptions = (archived ? facets.data?.archived_projects : facets.data?.projects) ?? [];

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

  // 搜索防抖：输入停止 250ms 才进 filter（此前每敲一个键就打一次后端分页）
  useEffect(() => {
    const timer = window.setTimeout(() => {
      const value = searchDraft.trim() || null;
      setFilter((f) => (bodyMode ? { ...f, body: value, q: null } : { ...f, q: value, body: null }));
    }, 250);
    return () => window.clearTimeout(timer);
  }, [searchDraft, bodyMode]);

  // 命令面板「定位会话」：选中并打开预览（行不在当前筛选里也能预览）
  useEffect(
    () =>
      onFocusSession((key) => {
        setSelected(key);
      }),
    [],
  );

  // 键盘：Ctrl / Cmd+F 聚焦搜索框；↑ / ↓ 在已加载的行间移动预览选中
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') {
        event.preventDefault();
        searchRef.current?.focus();
        return;
      }
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      const target = event.target as HTMLElement | null;
      if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
      if (items.length === 0) return;
      event.preventDefault();
      const at = selected ? items.findIndex((r) => rowKey(r) === selected) : -1;
      const next = event.key === 'ArrowDown' ? Math.min(at + 1, items.length - 1) : Math.max(at - 1, 0);
      setSelected(rowKey(items[next]));
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [items, selected]);

  const preview = useQuery<Preview>({
    queryKey: ['preview', selected],
    queryFn: () => queryCmd('session_preview', { key: selected! }),
    enabled: !!selected,
  });

  // 自动归档档位存在后端 kv 里：换机器/重启都跟着走，不落 localStorage（仅活跃页）
  const retention = useQuery<number>({
    queryKey: ['retention'],
    queryFn: () => queryCmd('session_retention'),
    enabled: !archived,
  });

  const { busy, run: runBusy } = useBusy();

  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['facets'] });
    qc.invalidateQueries({ queryKey: ['sessions'] });
    qc.invalidateQueries({ queryKey: ['session-stats'] });
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
      toggle(checked, setChecked, rowKey(row));
      return;
    }
    setSelected(rowKey(row));
  };

  const checkedRows = items.filter((row) => checked.has(rowKey(row)));
  const visibleChecked = checkedRows.filter((row) => row.state === 'visible');
  const specialChecked = checkedRows.filter((row) => PURGEABLE.has(row.state));
  const hasSel = checked.size > 0;

  const pageKeys = items.map(rowKey);
  const allOnPage = pageKeys.length > 0 && pageKeys.every((key) => checked.has(key));
  const someOnPage = !allOnPage && pageKeys.some((key) => checked.has(key));
  const headRef = useRef<HTMLInputElement | null>(null);
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

  /** 一键全选：按当前筛选选中全部（不只是已加载的这页）。 */
  const selectAllMatching = async () => {
    const keys = await runCmd('session_keys', { filter });
    if (keys === undefined) return;
    setChecked(new Set(keys));
    toast(`已选中 ${keys.length} 条`);
  };

  // ------------------------------------------------------------- 批量动作

  const archiveSelected = (on: boolean) =>
    runBusy(on ? 'archive' : 'unarchive', async () => {
      const keys = checkedRows.map(rowKey);
      if (keys.length === 0) return;
      if (on && visibleChecked.length === 0) {
        toast('只有「可见」状态的会话可以归档；特殊状态请用「彻底删除」');
        return;
      }
      const count = await runCmd('session_archive', { keys, on });
      if (count === undefined) return;
      toast(on ? `已归档 ${count} 条会话` : `已取消归档 ${count} 条会话，已回到活跃列表`);
      setChecked(new Set());
      setSelected(null);
      refresh();
    });

  const purgeSelected = () =>
    runBusy('purge', async () => {
      const keys = (specialChecked.length > 0 ? specialChecked : checkedRows).map(rowKey);
      if (keys.length === 0) return;
      const clients = await queryCmd('session_running_clients', { keys }).catch(
        () => [] as string[],
      );
      const ok = await confirmAction({
        title: '彻底删除会话',
        message:
          `彻底删除选中的 ${keys.length} 条会话？\n\n` +
          '会从对应客户端真正删掉数据，不可恢复。' +
          (clients.length ? `\n\n需要先临时关闭：${clients.join('、')}（删完自动重启）` : ''),
        danger: true,
      });
      if (!ok) return;
      if ((await runCmd('session_purge', { keys })) === undefined) return;
      setChecked(new Set());
      setSelected(null);
    });

  const hideSelected = (on: boolean) =>
    runBusy('hide', async () => {
      const keys = checkedRows.map(rowKey);
      if (keys.length === 0) return;
      if (on) {
        const ok = await confirmAction({
          title: '本地隐藏会话',
          message:
            `隐藏选中的 ${keys.length} 条会话？\n\n` +
            '隐藏只影响本工具的列表，客户端不受影响；「已隐藏」视图里可以随时恢复。',
        });
        if (!ok) return;
      }
      const count = await runCmd('session_hide', { keys, on });
      if (count === undefined) return;
      toast(on ? `已隐藏 ${count} 条会话` : `已取消隐藏 ${count} 条会话`);
      setChecked(new Set());
      setSelected(null);
      refresh();
    });

  const pinSelected = (on: boolean) =>
    runBusy('pin', async () => {
      const keys = checkedRows.map(rowKey);
      if (keys.length === 0) return;
      const count = await runCmd('session_pin', { keys, on });
      if (count === undefined) return;
      toast(on ? `已置顶 ${count} 条会话` : `已取消置顶 ${count} 条会话`);
      refresh();
    });

  const pinRow = (row: SessionRow, on: boolean) =>
    void runCmd('session_pin', { keys: [rowKey(row)], on }).then((count) => {
      if (count === undefined) return;
      refresh();
    });

  const exportSelected = (format: 'markdown' | 'json') =>
    runBusy('export', async () => {
      const keys = (checkedRows.length > 0 ? checkedRows : selected ? items.filter((r) => rowKey(r) === selected) : []).map(rowKey);
      if (keys.length === 0) {
        toast('先勾选要导出的会话（Ctrl / Cmd + 点击行可多选）');
        return;
      }
      const report = await runCmd('session_export', { keys, format });
      if (report === undefined) return;
      toast(
        `已导出 ${report.sessions} 条会话（${report.messages} 条消息）\n${report.path}`,
        6000,
      );
    });

  const scan = () =>
    runBusy('scan', async () => {
      const report = await runCmd('session_scan');
      if (report === undefined) return;
      // 来源错误由全局事件桥统一提示（并做去重），这里只报扫描结果
      toast(`扫描完成：写入 ${report.rows} 条`);
      refresh();
    });

  // 未命名治理：按当前（未命名）筛选取 keys，向后端要标题建议
  const openSuggestTitles = () =>
    runBusy('suggest', async () => {
      const keys = await runCmd('session_keys', { filter: { ...filter, unnamed: true } });
      if (keys === undefined || keys.length === 0) {
        toast('当前筛选下没有未命名会话');
        return;
      }
      const capped = keys.slice(0, 50);
      const suggestions = await fetchSuggestions(capped);
      if (suggestions === null) return;
      // 建议里带上客户端原标题（行在当前列表里才找得到；找不到显示会话 id 尾段）
      const withCurrent = suggestions.map((item) => {
        const row = items.find((r) => rowKey(r) === item.key);
        return {
          ...item,
          current: row?.title ?? `…${item.key.split('|').pop()?.slice(-8) ?? ''}`,
        };
      });
      setSuggestItems(withCurrent);
    });

  // ------------------------------------------------------------- 右键菜单

  const menuItemsFor = (row: SessionRow): MenuItem[] => {
    // 右键的行已在选中集里 = 动作作用于整批；否则只作用于这一行
    const inBatch = checked.has(rowKey(row));
    const targets = inBatch ? checkedRows : [row];
    const keys = targets.map(rowKey);
    const isArchivedRow = row.state === 'archived';
    return [
      {
        label: '打开预览',
        onSelect: () => setSelected(rowKey(row)),
      },
      ...(archived
        ? []
        : [
            {
              label: row.pinned ? '取消置顶' : '置顶',
              onSelect: () => pinRow(row, !row.pinned),
            },
          ]),
      {
        label: '重命名…',
        onSelect: () => setRenameTarget(row),
      },
      {
        label: '设置标签…',
        onSelect: () => setTagsTarget(row),
      },
      ...(isArchivedRow
        ? [
            {
              label: '取消归档',
              onSelect: () =>
                void runCmd('session_archive', { keys, on: false }).then((count) => {
                  if (count === undefined) return;
                  toast(`已取消归档 ${count} 条会话，已回到活跃列表`);
                  setChecked(new Set());
                  refresh();
                }),
            },
          ]
        : [
            {
              label: '归档',
              disabled: row.state !== 'visible',
              hint: row.state !== 'visible' ? '只有可见会话能归档' : undefined,
              onSelect: () => {
                void runCmd('session_archive', { keys, on: true }).then((count) => {
                  if (count === undefined) return;
                  toast(`已归档 ${count} 条会话`);
                  setChecked(new Set());
                  refresh();
                });
              },
            },
          ]),
      ...(archived
        ? []
        : [
            {
              // filter.hidden 视图里所有行都是隐藏态，菜单方向随之翻转
              label: filter.hidden ? '取消隐藏' : '隐藏',
              onSelect: () => void runCmd('session_hide', { keys, on: !filter.hidden }).then((count) => {
                if (count === undefined) return;
                toast(filter.hidden ? '已取消隐藏' : '已隐藏（在「已隐藏」chip 里找回）');
                setChecked(new Set());
                refresh();
              }),
            },
          ]),
      {
        label: '复制标题',
        onSelect: () => {
          void navigator.clipboard.writeText(row.title || row.session_id);
          toast('标题已复制');
        },
      },
      {
        label: '导出 Markdown',
        onSelect: () => void runCmd('session_export', { keys, format: 'markdown' }).then((report) => {
          if (report) toast(`已导出：${report.path}`, 6000);
        }),
      },
      {
        label: '导出 JSON',
        onSelect: () => void runCmd('session_export', { keys, format: 'json' }).then((report) => {
          if (report) toast(`已导出：${report.path}`, 6000);
        }),
      },
      {
        label: '打开正文所在文件夹',
        onSelect: () =>
          void runCmd('session_open_body', { key: rowKey(row) }).then((opened) => {
            if (opened === undefined) return;
            if (!opened) toast('该来源的正文在客户端数据库里，没有可直接打开的文件');
          }),
      },
      {
        label: '打开客户端',
        onSelect: () =>
          void runCmd('session_open_client', { key: rowKey(row) }).then((opened) => {
            if (opened === undefined) return;
            if (!opened) toast('该来源没有可启动的独立客户端');
          }),
      },
      {
        label: '彻底删除',
        danger: true,
        disabled: row.state === 'visible' && !archived,
        hint: row.state === 'visible' && !archived ? '先归档才能彻底删除' : undefined,
        onSelect: () => {
          void (async () => {
            const clients = await queryCmd('session_running_clients', { keys }).catch(
              () => [] as string[],
            );
            const ok = await confirmAction({
              title: '彻底删除会话',
              message:
                `彻底删除选中的 ${keys.length} 条会话？\n\n` +
                '会从对应客户端真正删掉数据，不可恢复。' +
                (clients.length ? `\n\n需要先临时关闭：${clients.join('、')}（删完自动重启）` : ''),
              danger: true,
            });
            if (!ok) return;
            if ((await runCmd('session_purge', { keys })) === undefined) return;
            setChecked(new Set());
            setSelected(null);
            refresh();
          })();
        },
      },
    ];
  };

  // ------------------------------------------------------------- 分组

  const groups = useMemo(() => groupRows(items, group), [items, group]);

  // 分组时把被分组的那一列藏掉：信息已经在组头上，不必每行再重复一遍。
  // 归档列表整表都是归档件，列定义里本来就没有「状态」列。
  const columns = (archived ? ARCHIVED_COLUMNS : SESSION_COLUMNS).filter(
    (column) => column.key !== group,
  );

  const total = listQuery.data?.pages[0]?.total ?? 0;
  const facetsData = facets.data;
  const hasActiveFilters =
    filter.source !== null ||
    filter.project !== null ||
    filter.q !== null ||
    filter.body !== null ||
    filter.unnamed ||
    filter.pinned ||
    filter.hidden ||
    (filter.tags?.length ?? 0) > 0;

  return (
    <div className="flex h-[calc(100vh-118px)] flex-col gap-3 overflow-auto">
      <div className={selected ? 'flex min-h-0 flex-1 flex-col gap-3 lg:flex-row' : 'flex flex-col gap-3'}>
        {/* ---------------------------------------------------------- 列表区 */}
        <section className={selected ? 'flex min-h-0 min-w-0 flex-1 flex-col gap-2' : 'flex min-h-0 flex-col gap-2'}>
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-semibold">{archived ? '已归档会话' : '活跃会话'}</h2>
            {archived && <Tag tone="warn">已归档</Tag>}
            <span className="text-xs text-muted">
              共 {total} 条
              {archived ? ' · 归档只写本地标记，取消归档即回到活跃列表' : ''}
            </span>
            {!archived && facetsData && (
              <button
                className="text-xs text-muted hover:text-fg"
                onClick={() => navigateArchive()}
                title="归档只写本地标记，取消归档可恢复"
              >
                已归档（{archiveCount(facetsData)}）→
              </button>
            )}
            {archived && (
              <button
                className="text-xs text-muted hover:text-fg"
                onClick={() => window.dispatchEvent(new CustomEvent('aim:navigate', { detail: 'sessions' }))}
              >
                ← 回活跃列表
              </button>
            )}
          </div>

          {/* 筛选条：一份列表，维度全在下面两行里。
              第一行挑子集（平台 / 状态 / 项目 / 标签 + 搜索 + 快筛），
              第二行只改「怎么摆」和扫描入口。 */}
          <div className="flex flex-wrap items-center gap-3 border-b border-line pb-2 text-sm">
            <label className="flex items-center gap-1 text-muted">
              平台
              <Select
                value={filter.source ?? ''}
                onChange={(e) => setFilter((f) => ({ ...f, source: e.target.value || null }))}
              >
                <option value="">全部</option>
                {platformOptions.map((s) => (
                  <option key={s.value} value={s.value}>
                    {s.label}（{s.count}）
                  </option>
                ))}
              </Select>
            </label>
            {!archived && (
              <label className="flex items-center gap-1 text-muted">
                状态
                <Select
                  value={filter.state ?? ''}
                  onChange={(e) => setFilter((f) => ({ ...f, state: e.target.value || null }))}
                >
                  <option value="">全部</option>
                  {(facetsData?.states ?? []).map((s) => (
                    <option key={s.value} value={s.value}>
                      {s.label}
                    </option>
                  ))}
                </Select>
              </label>
            )}
            <label className="flex items-center gap-1 text-muted">
              项目
              <Select
                value={filter.project ?? ''}
                onChange={(e) => setFilter((f) => ({ ...f, project: e.target.value || null }))}
              >
                <option value="">全部</option>
                {projectOptions.map((p) => (
                  <option key={p.value} value={p.value}>
                    {p.label}（{p.count}）
                  </option>
                ))}
              </Select>
            </label>
            {(facetsData?.tags.length ?? 0) > 0 && (
              <label className="flex items-center gap-1 text-muted">
                标签
                <Select
                  value={filter.tags?.[0] ?? ''}
                  onChange={(e) => setFilter((f) => ({ ...f, tags: e.target.value ? [e.target.value] : [] }))}
                >
                  <option value="">全部</option>
                  {(facetsData?.tags ?? []).map((t) => (
                    <option key={t.value} value={t.value}>
                      {t.label}（{t.count}）
                    </option>
                  ))}
                </Select>
              </label>
            )}
            <label className="flex items-center gap-1 text-muted">
              搜索
              <Input
                ref={searchRef}
                className="w-44"
                value={searchDraft}
                onChange={(e) => setSearchDraft(e.target.value)}
                placeholder={bodyMode ? '正文关键词（≥3 字更快）' : '标题 / 项目 / 平台'}
              />
            </label>
            <FilterChip
              active={bodyMode}
              label="正文"
              title="按会话正文全文搜索（FTS，中文按子串匹配）；关闭时搜索标题与项目"
              onClick={() => {
                setBodyMode((v) => !v);
                setSearchDraft('');
              }}
            />
            {!archived && (
              <>
                <FilterChip
                  active={filter.unnamed}
                  label={`未命名${facetsData?.unnamed ? `（${facetsData.unnamed}）` : ''}`}
                  title="只看标题是「（未命名会话）」占位文案的会话"
                  onClick={() => setFilter((f) => ({ ...f, unnamed: !f.unnamed }))}
                />
                <FilterChip
                  active={filter.pinned}
                  label={`置顶${facetsData?.pinned ? `（${facetsData.pinned}）` : ''}`}
                  title="只看置顶的会话；置顶的会话也永远排在列表最前"
                  onClick={() => setFilter((f) => ({ ...f, pinned: !f.pinned }))}
                />
                <FilterChip
                  active={filter.hidden}
                  label={`已隐藏${facetsData?.hidden ? `（${facetsData.hidden}）` : ''}`}
                  title="本地隐藏的会话（含 Trae 家族远端删不掉的删除墓碑），可从这里取消隐藏"
                  onClick={() => setFilter((f) => ({ ...f, hidden: !f.hidden }))}
                />
              </>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-3 text-sm">
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
              <Select value={group} onChange={(e) => setGroup(e.target.value as GroupMode)}>
                <option value="none">不分组</option>
                <option value="source">按平台</option>
                <option value="project">按项目</option>
                {!archived && <option value="state">按状态</option>}
              </Select>
            </label>
            {!archived && (
              <label
                className="flex items-center gap-1 text-muted"
                title="超过这个天数没活动的会话，扫描时自动归档"
              >
                自动归档
                <Select
                  value={String(retention.data ?? 0)}
                  onChange={(e) => {
                    const days = Number(e.target.value);
                    void queryCmd('session_retention_set', { days })
                      .then((saved) => {
                        qc.setQueryData(['retention'], saved);
                        toast(saved === 0 ? '已关闭自动归档' : `超过 ${saved} 天没动的会话会自动归档`);
                        refresh();
                      })
                      .catch((e) => toast(errText(e)));
                  }}
                >
                  {RETENTION_CHOICES.map((choice) => (
                    <option key={choice.days} value={choice.days}>
                      {choice.label}
                    </option>
                  ))}
                </Select>
              </label>
            )}
            <span className="flex-1" />
            {!archived && (
              <Button size="sm" onClick={scan} loading={busy === 'scan'} disabled={!!busy}>
                重新扫描
              </Button>
            )}
          </div>

          {/* 已隐藏视图的口径说明：取消隐藏=恢复显示（Trae 家族的删除就是隐藏墓碑） */}
          {filter.hidden && (
            <Notice>
              这些会话在本地被隐藏（Trae 家族的「删除」也是隐藏墓碑——远端删不掉）。取消隐藏 = 恢复显示。
            </Notice>
          )}

          {/* 未命名治理：只在未命名快筛激活时出现 */}
          {!archived && filter.unnamed && (
            <div className="flex items-center gap-2 text-sm">
              <Button
                size="sm"
                variant="ghost"
                loading={busy === 'suggest'}
                disabled={!!busy}
                onClick={openSuggestTitles}
                title="取每条会话的第一句提问当标题建议，人工确认后写入本地命名"
              >
                为未命名会话生成标题建议
              </Button>
              <span className="text-xs text-muted">建议写入的是本工具的显示名，客户端不受影响。</span>
            </div>
          )}

          {/* 选择操作单独一行：左边说清楚「选了几条」，右边才是动作 */}
          <div className="flex flex-wrap items-center gap-2 text-sm">
            {checked.size > 0 && <span className="text-xs text-muted">已选 {checked.size} 条</span>}
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void selectAllMatching()}
              disabled={total === 0 || !!busy}
              title="按当前筛选选中全部会话（不只是当前页）"
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
            <span className="text-xs text-muted">Ctrl / Cmd + 点击行可多选；右键行有更多动作</span>
            <span className="flex-1" />
            {archived ? (
              <>
                <Button
                  size="sm"
                  disabled={!hasSel || !!busy}
                  loading={busy === 'unarchive'}
                  onClick={() => archiveSelected(false)}
                  title="取消归档后回到活跃列表"
                >
                  取消归档
                </Button>
                <Button
                  size="sm"
                  variant="danger"
                  disabled={!hasSel || !!busy}
                  loading={busy === 'purge'}
                  onClick={purgeSelected}
                  title="从客户端真正删除，不可恢复"
                >
                  彻底删除
                </Button>
              </>
            ) : (
              <>
                {filter.hidden ? (
                  <Button
                    size="sm"
                    disabled={!hasSel || !!busy}
                    loading={busy === 'hide'}
                    onClick={() => hideSelected(false)}
                    title="取消隐藏后回到活跃列表"
                  >
                    取消隐藏
                  </Button>
                ) : (
                  <>
                    <Button
                      size="sm"
                      disabled={!hasSel || visibleChecked.length === 0 || !!busy}
                      loading={busy === 'archive'}
                      onClick={() => archiveSelected(true)}
                      title="归档后不再出现在活跃列表；可见会话必须先归档才能彻底删除"
                    >
                      归档{visibleChecked.length > 0 ? `（${visibleChecked.length}）` : ''}
                    </Button>
                    <Button
                      size="sm"
                      disabled={!hasSel || !!busy}
                      loading={busy === 'pin'}
                      onClick={() => pinSelected(true)}
                      title="置顶：排在列表最前，不随排序变化"
                    >
                      置顶
                    </Button>
                    <Button
                      size="sm"
                      disabled={!hasSel || !!busy}
                      loading={busy === 'hide'}
                      onClick={() => hideSelected(true)}
                      title="本地隐藏：列表不再出现，可从「已隐藏」chip 找回"
                    >
                      隐藏
                    </Button>
                  </>
                )}
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={!hasSel || !!busy}
                  loading={busy === 'export'}
                  onClick={() => exportSelected('markdown')}
                  title="导出选中会话的正文为 Markdown（输出到数据目录 exports/）"
                >
                  导出
                </Button>
                {specialChecked.length > 0 && (
                  <Button
                    size="sm"
                    variant="danger"
                    disabled={!hasSel || !!busy}
                    loading={busy === 'purge'}
                    onClick={purgeSelected}
                    title="特殊状态（已删除 / 云端 / 旧账号 / 孤立）可直接彻底删除"
                  >
                    彻底删除（{specialChecked.length}）
                  </Button>
                )}
              </>
            )}
          </div>

          <div className="min-h-[140px] flex-1 overflow-auto">
            {listQuery.isLoading ? (
              // 骨架行必须待在 <tbody> 里：直接塞进 div 会被浏览器搬出表格
              <table className="w-full table-fixed border-collapse">
                <tbody>
                  <SkeletonRows rows={8} cols={(archived ? ARCHIVED_COLUMNS : SESSION_COLUMNS).length + 1} />
                </tbody>
              </table>
            ) : items.length === 0 ? (
              <EmptyState
                archived={archived}
                hasFilters={hasActiveFilters}
                total={total}
                onClear={() =>
                  setFilter((f) => ({
                    ...f,
                    source: null,
                    project: null,
                    q: null,
                    body: null,
                    unnamed: false,
                    pinned: false,
                    hidden: false,
                    tags: [],
                  }))
                }
                onScan={scan}
                scanning={busy === 'scan'}
              />
            ) : group === 'none' ? (
              <DataTable<SessionRow>
                className="w-full table-fixed border-collapse"
                stickyHead
                columns={columns}
                rows={items}
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
                        checked={checked.has(rowKey(row))}
                        onChange={() => toggle(checked, setChecked, rowKey(row))}
                      />
                    </span>
                  ),
                }}
                onRowClick={rowClick}
                onRowContextMenu={(row, event) => {
                  event.preventDefault();
                  // 右键未选中的行：把选中集换成这一行，动作语义直观
                  if (!checked.has(rowKey(row))) {
                    setChecked(new Set([rowKey(row)]));
                  }
                  setMenu({ x: event.clientX, y: event.clientY, row });
                }}
                rowClassName={(row) =>
                  `group cursor-pointer ${selected === rowKey(row) ? 'bg-hover' : 'hover:bg-hover'}`
                }
              />
            ) : (
              groups.map((g) => (
                <div key={g.key} className="mb-2">
                  <button
                    type="button"
                    className="flex w-full cursor-pointer items-center gap-2 border-b border-line bg-hover px-2 py-1 text-left text-xs font-semibold"
                    onClick={() => toggle(collapsed, setCollapsed, g.key)}
                  >
                    <span>{collapsed.has(g.key) ? '▸' : '▾'}</span>
                    <span>{g.label}</span>
                    <span className="text-muted">{g.rows.length} 条</span>
                  </button>
                  {!collapsed.has(g.key) && (
                    <DataTable<SessionRow>
                      // 与不分组时同一套列宽：分组表格各自成表，auto 布局会让
                      // 标题特别长的那一组把整表撑宽、右侧列被挤出容器
                      className="w-full table-fixed border-collapse"
                      dense
                      columns={columns}
                      rows={g.rows}
                      rowKey={rowKey}
                      empty={null}
                      lead={{
                        width: 32,
                        cell: (row) => (
                          <span onClick={(e) => e.stopPropagation()}>
                            <Checkbox
                              checked={checked.has(rowKey(row))}
                              onChange={() => toggle(checked, setChecked, rowKey(row))}
                            />
                          </span>
                        ),
                      }}
                      onRowClick={rowClick}
                      onRowContextMenu={(row, event) => {
                        event.preventDefault();
                        if (!checked.has(rowKey(row))) {
                          setChecked(new Set([rowKey(row)]));
                        }
                        setMenu({ x: event.clientX, y: event.clientY, row });
                      }}
                      rowClassName={(row) =>
                        `group cursor-pointer ${selected === rowKey(row) ? 'bg-hover' : 'hover:bg-hover'}`
                      }
                    />
                  )}
                </div>
              ))
            )}
            <div ref={sentinel} className="h-px" />
            {isFetchingNextPage && <div className="p-2 text-center text-xs text-muted">加载中…</div>}
            {!hasNextPage && items.length > 0 && (
              <div className="p-2 text-center text-xs text-muted">共 {items.length} 条，到底了</div>
            )}
          </div>
        </section>

        {/* ---------------------------------------------------------- 详情分栏 */}
        {/* 宽屏挂在列表右侧（42%），窄屏回落到列表下方——同一块 DOM，断点切换 */}
        {selected && (
          <aside className="max-h-[40%] w-full flex-none overflow-auto rounded-lg border border-line bg-card p-3 lg:max-h-none lg:w-[42%] lg:max-w-[560px]">
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
                  <Button
                    size="sm"
                    variant="ghost"
                    title="关闭预览"
                    onClick={() => setSelected(null)}
                  >
                    ×
                  </Button>
                </div>
                {preview.data.truncated && (
                  <Notice className="mb-2">
                    正文太长，只显示头尾部分（共 {preview.data.total} 条，略去 {preview.data.dropped} 条）。
                  </Notice>
                )}
                {preview.data.note && (
                  <Notice tone="warn" className="mb-2">
                    {preview.data.note}
                  </Notice>
                )}
                <div className="mt-2 border-t border-dashed border-line pt-2">
                  <PreviewBody
                    messages={preview.data.messages}
                    highlight={filter.q ?? filter.body ?? undefined}
                  />
                </div>
              </>
            )}
          </aside>
        )}
      </div>

      {/* 右键菜单 */}
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItemsFor(menu.row)}
          onClose={() => setMenu(null)}
        />
      )}

      {/* 编辑弹窗 */}
      {renameTarget && (
        <RenameDialog
          keyId={rowKey(renameTarget)}
          current={renameTarget.title}
          renamed={renameTarget.renamed}
          onClose={() => setRenameTarget(null)}
          onDone={refresh}
        />
      )}
      {tagsTarget && (
        <TagsDialog
          keyId={rowKey(tagsTarget)}
          current={tagsFromFacets(facets.data, tagsTarget)}
          allTags={(facets.data?.tags ?? []).map((t) => t.value)}
          onClose={() => setTagsTarget(null)}
          onDone={refresh}
        />
      )}
      {suggestItems && (
        <SuggestTitlesDialog
          items={suggestItems}
          onClose={() => setSuggestItems(null)}
          onDone={() => refresh()}
        />
      )}
    </div>
  );
}

// ------------------------------------------------------------------ 小组件

/** 快筛 chip：未命名 / 置顶 / 已隐藏 / 正文。 */
function FilterChip({
  active,
  label,
  title,
  onClick,
}: {
  active: boolean;
  label: string;
  title?: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`rounded-md px-2 py-1 text-sm ${
        active
          ? 'bg-accent font-medium text-on-accent'
          : 'border border-line text-muted hover:bg-hover hover:text-fg'
      }`}
      onClick={onClick}
      title={title}
    >
      {label}
    </button>
  );
}

/** 空态：区分「真的没有」与「筛没了」，给下一步动作而不是一行灰字。 */
function EmptyState({
  archived,
  hasFilters,
  total,
  onClear,
  onScan,
  scanning,
}: {
  archived: boolean;
  hasFilters: boolean;
  total: number;
  onClear: () => void;
  onScan: () => void;
  scanning: boolean;
}) {
  if (hasFilters) {
    return (
      <div className="flex flex-col items-center gap-2 py-6">
        <p className="text-sm text-muted">没有符合条件的会话。</p>
        <Button size="sm" variant="ghost" onClick={onClear}>
          清空筛选
        </Button>
      </div>
    );
  }
  return (
    <div className="flex flex-col items-center gap-2 py-6">
      <p className="text-sm text-muted">
        {archived
          ? '还没有已归档的会话。'
          : total === 0
            ? '还没有会话记录。装好客户端并登录后，会话会在扫描时自动出现。'
            : '没有符合条件的会话。'}
      </p>
      {!archived && (
        <Button size="sm" variant="ghost" onClick={onScan} loading={scanning}>
          重新扫描
        </Button>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ 辅助

/** 归档总数：facets.archived（全部已归档行，与设置页入口同口径）。 */
function archiveCount(facets: Facets): number {
  return facets.archived;
}

/** 某会话当前挂的标签：facets 只给聚合数，单会话的标签取列表行——
 *  行上没有，就退化为空（编辑弹窗里再点一次就是全量替换，无副作用）。 */
function tagsFromFacets(_facets: Facets | undefined, _row: SessionRow): string[] {
  return [];
}

function navigateArchive() {
  window.dispatchEvent(new CustomEvent('aim:navigate', { detail: 'sessions-archive' }));
}

