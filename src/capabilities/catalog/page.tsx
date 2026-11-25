import { useEffect, useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronLeft, ChevronRight, Grid2X2, List, RotateCcw } from 'lucide-react';
import { Button, Card, Input, Notice, Section, Select, SkeletonBlock, StateBlock } from '@/components/ui';
import { DataTable } from '@/components/ui/table';
import { errMsg, queryCmd } from '@/lib/cmd';
import { listen } from '@/lib/events';
import type { CatalogFacetCount, CatalogMedia, CatalogOverview, CatalogQuery } from '@/lib/types';
import { CARD_GRID, MediaCard } from './cards';
import { catalogColumns, releaseColumns } from './columns';
import { CatalogDetailDrawer } from './detail';
import { KIND_ORDER, MEDIA_STATUSES, formatDay, isISODate, kindLabel, mondayISO, regionLabel, shiftISO, todayISO, weekRange } from './helpers';
import { CatalogSources } from './sources';

const TABS = [
  { id: 'library', label: '媒体库' }, { id: 'daily', label: '每日发行' },
  { id: 'weekly', label: '每周发行' }, { id: 'overview', label: '概览' }, { id: 'ingestion', label: '采集管理' },
] as const;
type Tab = (typeof TABS)[number]['id'];
const EMPTY_FILTERS = { kind: '', region: '', genre: '', platform: '', status: '', year: '', rating: '' };
type Filters = typeof EMPTY_FILTERS;
const WEEKDAYS = ['一', '二', '三', '四', '五', '六', '日'];

function FacetFilter({ label, value, items, disabled, onChange, region = false }: {
  label: string; value: string; items: CatalogFacetCount[]; disabled: boolean; onChange: (value: string) => void; region?: boolean;
}) {
  return <Select aria-label={label} value={value} disabled={disabled} title={disabled ? '这些组合筛选适用于媒体库与发行日历' : label} onChange={(event) => onChange(event.target.value)}>
    <option value="">全部{label}</option>
    {items.map((item) => <option key={item.key} value={item.key}>{region ? regionLabel(item.key) : item.label || item.key}（{item.count}）</option>)}
  </Select>;
}

export function CatalogPage() {
  const qc = useQueryClient();
  const [tab, setTab] = useState<Tab>('library');
  const [search, setSearch] = useState('');
  const [filters, setFilters] = useState<Filters>(EMPTY_FILTERS);
  const [date, setDate] = useState(todayISO);
  const [weekday, setWeekday] = useState<number | null>(null);
  const [sort, setSort] = useState('newest');
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(24);
  const [layout, setLayout] = useState<'grid' | 'table'>('grid');
  const [mediaKey, setMediaKey] = useState<string | null>(null);
  const isList = ['library', 'daily', 'weekly'].includes(tab);
  const isSchedule = tab === 'daily' || tab === 'weekly';
  const range = weekRange(date);
  const dateValid = isISODate(date);
  const facetsQ = useQuery({ queryKey: ['catalog', 'facets'], queryFn: () => queryCmd('catalog_facets') });
  const query = useMemo<CatalogQuery>(() => ({
    search: search.trim() || null, kind: filters.kind || null, region: filters.region || null,
    genre: filters.genre || null, platform: filters.platform || null, status: filters.status || null,
    year: filters.year ? Number(filters.year) : null, min_rating: filters.rating ? Number(filters.rating) : null,
    start: tab === 'daily' ? date : tab === 'weekly' ? range.start : null,
    end: tab === 'daily' ? shiftISO(date, 1) : tab === 'weekly' ? range.end : null,
    weekday: tab === 'weekly' ? weekday : null, sort, page, page_size: pageSize,
  }), [search, filters, tab, date, range.start, range.end, weekday, sort, page, pageSize]);
  const libraryQ = useQuery({ queryKey: ['catalog', 'library', query], queryFn: () => queryCmd('catalog_library', { query }), enabled: tab === 'library' });
  const releasesQ = useQuery({ queryKey: ['catalog', 'releases', query], queryFn: () => queryCmd('catalog_releases', { query }), enabled: isSchedule && dateValid });
  const overviewQ = useQuery({ queryKey: ['catalog', 'overview'], queryFn: () => queryCmd('catalog_overview'), enabled: tab === 'overview' });
  const columns = useMemo(catalogColumns, []);
  const scheduleColumns = useMemo(releaseColumns, []);
  const activeQ = tab === 'library' ? libraryQ : releasesQ;

  // SSE 只负责使本地查询失效；页面不发起首次采集，也不另建轮询调度。
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<unknown>('catalog:updated', () => { void qc.invalidateQueries({ queryKey: ['catalog'] }); }).then((off) => {
      if (disposed) off(); else unlisten = off;
    });
    return () => { disposed = true; unlisten?.(); };
  }, [qc]);

  const changeFilter = (key: keyof Filters, value: string) => { setFilters((old) => ({ ...old, [key]: value })); setPage(1); };
  const changeDate = (value: string) => { setDate(tab === 'weekly' && isISODate(value) ? mondayISO(value) : value); setPage(1); };
  const changeTab = (next: Tab) => {
    setTab(next); setPage(1); setWeekday(null);
    if (next === 'weekly' && dateValid) setDate(mondayISO(date));
    if (next === 'daily' || next === 'weekly') setSort('time');
    else if (next === 'library' && sort === 'time') setSort('newest');
  };
  const resetFilters = () => { setSearch(''); setFilters(EMPTY_FILTERS); setWeekday(null); setPage(1); };
  const hasFilters = !!search || Object.values(filters).some(Boolean) || weekday !== null;

  return <div className="flex flex-col gap-3">
    <Card className="overflow-hidden p-0">
      <div className="flex flex-wrap items-center gap-2 border-b border-line bg-sunken p-2">
        <div className="flex flex-wrap gap-1" aria-label="媒体库视图">
          {TABS.map((item) => <Button key={item.id} type="button" size="sm" variant={tab === item.id ? 'soft' : 'ghost'} aria-pressed={tab === item.id} onClick={() => changeTab(item.id)}>{item.label}</Button>)}
        </div>
        <Input type="search" aria-label="搜索作品名称或别名" placeholder="搜索作品名称、原名、别名" value={search} className="min-w-0 flex-1 sm:min-w-48" title="概览与采集管理不接受作品筛选；输入搜索会转到媒体库" onChange={(event) => { setSearch(event.target.value); setPage(1); if (!isList) changeTab('library'); }} />
        {isList && <div className="flex gap-1" aria-label="展示方式">
          <Button type="button" size="sm" variant={layout === 'grid' ? 'soft' : 'ghost'} aria-label="卡片视图" aria-pressed={layout === 'grid'} onClick={() => setLayout('grid')}><Grid2X2 size={13} aria-hidden /></Button>
          <Button type="button" size="sm" variant={layout === 'table' ? 'soft' : 'ghost'} aria-label="紧凑表格" aria-pressed={layout === 'table'} onClick={() => setLayout('table')}><List size={13} aria-hidden /></Button>
        </div>}
      </div>
      <div className="flex flex-col gap-2 p-2">
        <div className="flex flex-wrap gap-1" aria-label="作品类型">
          <Button type="button" size="sm" variant={!filters.kind ? 'soft' : 'ghost'} disabled={!isList} title={!isList ? '类型筛选适用于媒体库与日历' : undefined} aria-pressed={!filters.kind} onClick={() => changeFilter('kind', '')}>全部类型</Button>
          {KIND_ORDER.map((kind) => {
            const facet = facetsQ.data?.kinds.find((item) => item.key === kind);
            return <Button key={kind} type="button" size="sm" variant={filters.kind === kind ? 'soft' : 'ghost'} disabled={!isList} title={!isList ? '类型筛选适用于媒体库与日历' : facet ? '计数来自当前资料库，不代表来源全量覆盖' : '该类型暂未返回数据库计数'} aria-pressed={filters.kind === kind} onClick={() => changeFilter('kind', kind)}>{kindLabel(kind)}{facet ? `（${facet.count}）` : ''}</Button>;
          })}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <FacetFilter label="国家/地区" value={filters.region} items={facetsQ.data?.regions ?? []} disabled={!isList || facetsQ.isLoading} onChange={(value) => changeFilter('region', value)} region />
          <FacetFilter label="题材" value={filters.genre} items={facetsQ.data?.genres ?? []} disabled={!isList || facetsQ.isLoading} onChange={(value) => changeFilter('genre', value)} />
          <FacetFilter label="平台" value={filters.platform} items={facetsQ.data?.platforms ?? []} disabled={!isList || facetsQ.isLoading} onChange={(value) => changeFilter('platform', value)} />
          <Select aria-label="发行状态" value={filters.status} disabled={!isList} title={!isList ? '状态筛选适用于媒体库与日历' : '作品发行状态，不是观看状态'} onChange={(event) => changeFilter('status', event.target.value)}><option value="">全部发行状态</option>{Object.entries(MEDIA_STATUSES).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</Select>
          <Select aria-label="年份" value={filters.year} disabled={!isList || facetsQ.isLoading} title={!isList ? '年份筛选适用于媒体库与日历' : '年份来自数据库'} onChange={(event) => changeFilter('year', event.target.value)}><option value="">全部年份</option>{(facetsQ.data?.years ?? []).map((year) => <option key={year} value={year}>{year} 年</option>)}</Select>
          <Select aria-label="用户或观众评分下限，十分制" value={filters.rating} disabled={!isList} title="后端选择一个用户/观众评分换算为十分制；来源见条目。不是专业评论或综合评分" onChange={(event) => changeFilter('rating', event.target.value)}><option value="">用户/观众评分不限</option>{[6, 7, 8, 9].map((rating) => <option key={rating} value={rating}>用户/观众 ≥ {rating} / 10</option>)}</Select>
          {hasFilters && <Button type="button" size="sm" variant="ghost" onClick={resetFilters}><RotateCcw size={12} aria-hidden />清空筛选</Button>}
          {isList && <Select aria-label="排序" value={sort} className="ml-auto" onChange={(event) => { setSort(event.target.value); setPage(1); }}>{isSchedule && <option value="time">发行时间</option>}<option value="rating">用户/观众评分</option><option value="newest">{isSchedule ? '近期发行优先' : '最新收录'}</option><option value="title">标题</option>{!isSchedule && <option value="year">年份</option>}</Select>}
        </div>
        {isSchedule && <div className="flex flex-wrap items-center gap-2 border-t border-line pt-2">
          <Button type="button" size="sm" variant="ghost" aria-label={tab === 'weekly' ? '上一周' : '上一天'} disabled={!dateValid} title={!dateValid ? '请先选择有效日期' : undefined} onClick={() => changeDate(shiftISO(date, tab === 'weekly' ? -7 : -1))}><ChevronLeft size={14} aria-hidden /></Button>
          <Input type="date" aria-label={tab === 'weekly' ? '选择周一至周日的任一天' : '每日发行日期'} value={date} invalid={!dateValid} onChange={(event) => changeDate(event.target.value)} />
          <Button type="button" size="sm" variant="ghost" aria-label={tab === 'weekly' ? '下一周' : '下一天'} disabled={!dateValid} title={!dateValid ? '请先选择有效日期' : undefined} onClick={() => changeDate(shiftISO(date, tab === 'weekly' ? 7 : 1))}><ChevronRight size={14} aria-hidden /></Button>
          <Button type="button" size="sm" variant="ghost" onClick={() => changeDate(todayISO())}>{tab === 'weekly' ? '本周' : '今天'}</Button>
          {dateValid && <span className="text-xs tabular-nums text-fg2">{tab === 'weekly' ? `${formatDay(range.start)} — ${formatDay(range.last)}` : formatDay(date)}</span>}
          {tab === 'weekly' && <div className="flex flex-wrap gap-1" aria-label="星期筛选">
            <Button type="button" size="sm" variant={weekday === null ? 'soft' : 'ghost'} aria-pressed={weekday === null} onClick={() => { setWeekday(null); setPage(1); }}>全周</Button>
            {WEEKDAYS.map((label, index) => <Button key={label} type="button" size="sm" variant={weekday === index + 1 ? 'soft' : 'ghost'} aria-pressed={weekday === index + 1} aria-label={`只看周${label}`} onClick={() => { setWeekday(index + 1); setPage(1); }}>周{label}</Button>)}
          </div>}
        </div>}
      </div>
    </Card>
    {isList && facetsQ.isError && <Notice tone="warn">筛选项读取失败：{errMsg(facetsQ.error)} <Button type="button" size="sm" variant="ghost" onClick={() => void facetsQ.refetch()}>重试筛选项</Button></Notice>}
    {isList && (isSchedule && !dateValid ? <Notice tone="warn">请选择有效日期后查询发行日历。</Notice> : activeQ.isError ? <StateBlock kind="error" title="资料读取失败" detail={errMsg(activeQ.error)} onRetry={() => void activeQ.refetch()} /> : activeQ.isLoading ? <SkeletonBlock rows={8} /> : (
      <>
        {!activeQ.data?.items.length ? <StateBlock kind="empty" title={isSchedule ? '未采集到已确认更新，不代表当天无作品' : '没有符合条件的作品'} detail={isSchedule ? '日历仅显示来源已确认的发行记录；覆盖范围见采集管理。可切换日期、清空筛选或检查采集结果。' : '媒体库仅包含已采集元数据；可清空筛选或在采集管理检查来源覆盖。'} action={<Button type="button" size="sm" variant="soft" onClick={hasFilters ? resetFilters : () => changeTab('ingestion')}>{hasFilters ? '清空筛选' : '查看采集来源'}</Button>} /> : tab === 'library' ? layout === 'grid' ? (
          <div className={CARD_GRID}>{libraryQ.data?.items.map((media) => <MediaCard key={media.key} media={media} onOpen={setMediaKey} />)}</div>
        ) : <div className="overflow-x-auto rounded-lg border border-line bg-card"><DataTable columns={columns} rows={libraryQ.data?.items ?? []} rowKey={(row) => row.key} onRowClick={(row) => setMediaKey(row.key)} dense /></div> : layout === 'grid' ? (
          <div className={CARD_GRID}>{releasesQ.data?.items.map((row) => <MediaCard key={row.release.key} media={row.media} release={row.release} onOpen={setMediaKey} />)}</div>
        ) : <div className="overflow-x-auto rounded-lg border border-line bg-card"><DataTable columns={scheduleColumns} rows={releasesQ.data?.items ?? []} rowKey={(row) => row.release.key} onRowClick={(row) => setMediaKey(row.media.key)} dense /></div>}
        {activeQ.data && <Pagination total={activeQ.data.total} page={activeQ.data.page} pageSize={activeQ.data.page_size} loading={activeQ.isFetching} onPage={setPage} onPageSize={(size) => { setPageSize(size); setPage(1); }} />}
      </>
    ))}
    {tab === 'overview' && (overviewQ.isError ? <StateBlock kind="error" title="概览读取失败" detail={errMsg(overviewQ.error)} onRetry={() => void overviewQ.refetch()} /> : overviewQ.isLoading ? <SkeletonBlock rows={8} /> : overviewQ.data && <Overview data={overviewQ.data} onOpen={setMediaKey} />)}
    {tab === 'ingestion' && <CatalogSources />}
    <CatalogDetailDrawer mediaKey={mediaKey} onClose={() => setMediaKey(null)} />
  </div>;
}

function Pagination({ total, page, pageSize, loading, onPage, onPageSize }: {
  total: number; page: number; pageSize: number; loading: boolean; onPage: (page: number) => void; onPageSize: (size: number) => void;
}) {
  const pages = Math.max(1, Math.ceil(total / pageSize));
  return <div className="flex flex-wrap items-center justify-end gap-2 text-xs text-fg2">
    <span className="mr-auto tabular-nums" role="status">共 {total} 条 · 第 {page} / {pages} 页</span>
    <Select aria-label="每页条数" value={pageSize} onChange={(event) => onPageSize(Number(event.target.value))}>{[24, 48, 100].map((size) => <option key={size} value={size}>{size} 条 / 页</option>)}</Select>
    <Button type="button" size="sm" variant="ghost" disabled={loading || page <= 1} title={page <= 1 ? '已是第一页' : loading ? '正在查询' : undefined} aria-label="上一页" onClick={() => onPage(page - 1)}><ChevronLeft size={13} aria-hidden />上一页</Button>
    <Button type="button" size="sm" variant="ghost" disabled={loading || page >= pages} title={page >= pages ? '已是最后一页' : loading ? '正在查询' : undefined} aria-label="下一页" onClick={() => onPage(page + 1)}>下一页<ChevronRight size={13} aria-hidden /></Button>
  </div>;
}

function Overview({ data, onOpen }: { data: CatalogOverview; onOpen: (key: string) => void }) {
  const tiles = [ ['已收录', data.total], ['今日新增', data.today_added], ['今日确认发行', data.today_releases], ['本周新增', data.week_added], ['本周更新', data.week_updated] ] as const;
  const groups: Array<[string, CatalogMedia[]]> = [['最近作品', data.latest], ['即将发行', data.upcoming], ['高分作品', data.highly_rated]];
  return <div className="flex flex-col gap-3">
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-5">{tiles.map(([label, value]) => <Card key={label} className="p-3"><div className="text-xs text-muted">{label}</div><div className="mt-1 text-xl font-semibold tabular-nums">{value}</div></Card>)}</div>
    {groups.map(([label, media]) => <Section key={label} title={label} bodyClassName="p-3">{media.length ? <div className={CARD_GRID}>{media.map((item) => <MediaCard key={item.key} media={item} onOpen={onOpen} />)}</div> : <StateBlock kind="empty" title="暂无符合条件的已采集资料" />}</Section>)}
  </div>;
}

export default CatalogPage;
