import { useEffect, useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronLeft, ChevronRight, Grid2X2, List, RotateCcw, X } from 'lucide-react';
import { Button, Card, Field, Hint, Input, Notice, Picker, Section, SkeletonBlock, StateBlock, Tooltip } from '@/components/ui';
import { DataTable } from '@/components/ui/table';
import { errMsg, queryCmd } from '@/lib/cmd';
import { listen } from '@/lib/events';
import type { CatalogFacetCount, CatalogMedia, CatalogOverview, CatalogQuery } from '@/lib/types';
import { CARD_GRID, MediaCard } from './cards';
import { catalogColumns, releaseColumns } from './columns';
import { CatalogDetailDrawer } from './detail';
import { CatalogFollowView } from './follow';
import { KIND_ORDER, MEDIA_STATUSES, formatDay, isISODate, kindLabel, mondayISO, regionLabel, shiftISO, todayISO, weekRange } from './helpers';
import { CatalogSources } from './sources';

const SCOPES = [
  { id: 'domestic', label: '国内' },
  { id: 'foreign', label: '国外' },
] as const;
type Scope = (typeof SCOPES)[number]['id'];
const TABS = [
  { id: 'library', label: '媒体库' }, { id: 'follow', label: '我的追剧' }, { id: 'daily', label: '每日发行' },
  { id: 'weekly', label: '每周发行' }, { id: 'overview', label: '概览' }, { id: 'ingestion', label: '采集管理' },
] as const;
type Tab = (typeof TABS)[number]['id'];
const EMPTY_FILTERS = { kind: '', region: '', genre: '', platform: '', status: '', year: '', rating: '' };
type Filters = typeof EMPTY_FILTERS;
const WEEKDAYS = ['一', '二', '三', '四', '五', '六', '日'];
type FilterMode = 'any' | 'all';

function FacetFilter({ id, label, value, items, disabled, onChange, region = false }: {
  id: string; label: string; value: string; items: CatalogFacetCount[]; disabled: boolean; onChange: (value: string) => void; region?: boolean;
}) {
  return <Hint content={disabled ? '这些组合筛选适用于媒体库与发行日历' : label} focusable={false}>
    <Picker
      id={id}
      label={label}
      value={value}
      disabled={disabled}
      size="sm"
      className="w-full max-w-none"
      onValueChange={onChange}
      options={[
        { value: '', label: `全部${label}` },
        ...items.map((item) => ({ value: item.key, label: `${region ? regionLabel(item.key) : item.label || item.key}（${item.count}）` })),
      ]}
    />
  </Hint>;
}

/** 且/或切换：任一命中（or）还是同时包含（and）——同一筛选维度内部的组合方式。 */
function ModeToggle({ value, onChange, label }: { value: FilterMode; onChange: (next: FilterMode) => void; label: string }) {
  return <div className="flex gap-1" role="group" aria-label={`${label}的组合方式`}>
    <Button type="button" size="sm" variant={value === 'any' ? 'soft' : 'ghost'} aria-pressed={value === 'any'} onClick={() => onChange('any')}>任一</Button>
    <Button type="button" size="sm" variant={value === 'all' ? 'soft' : 'ghost'} aria-pressed={value === 'all'} onClick={() => onChange('all')}>全部</Button>
  </div>;
}

/** 已选值的小芯片，可单个移除。 */
function SelectedTags({ items, onRemove, removeLabel }: { items: string[]; onRemove: (value: string) => void; removeLabel: (value: string) => string }) {
  if (!items.length) return null;
  return <div className="flex flex-wrap gap-1" role="list">
    {items.map((item) => <span key={item} role="listitem" className="flex items-center gap-1 rounded bg-sunken px-1.5 py-0.5 text-xs text-fg">
      {item}
      <button type="button" className="text-muted hover:text-fg" aria-label={removeLabel(item)} onClick={() => onRemove(item)}><X size={11} aria-hidden /></button>
    </span>)}
  </div>;
}

export function CatalogPage() {
  const qc = useQueryClient();
  const [scope, setScope] = useState<Scope>('domestic');
  const [tab, setTab] = useState<Tab>('library');
  const [search, setSearch] = useState('');
  const [filters, setFilters] = useState<Filters>(EMPTY_FILTERS);
  const [genres, setGenres] = useState<string[]>([]);
  const [genreMode, setGenreMode] = useState<FilterMode>('any');
  const [cast, setCast] = useState<string[]>([]);
  const [castMode, setCastMode] = useState<FilterMode>('any');
  const [castDraft, setCastDraft] = useState('');
  const [yearFrom, setYearFrom] = useState('');
  const [yearTo, setYearTo] = useState('');
  const [date, setDate] = useState(todayISO);
  const [weekday, setWeekday] = useState<number | null>(null);
  const [sort, setSort] = useState('updated');
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(24);
  const [layout, setLayout] = useState<'grid' | 'table'>('table');
  const [mediaKey, setMediaKey] = useState<string | null>(null);
  const isList = ['library', 'daily', 'weekly'].includes(tab);
  const isScoped = isList || tab === 'overview';
  const isSchedule = tab === 'daily' || tab === 'weekly';
  const range = weekRange(date);
  const dateValid = isISODate(date);
  const query = useMemo<CatalogQuery>(() => ({
    search: search.trim() || null, kind: filters.kind || null, region: filters.region || null,
    genre: filters.genre || null, platform: filters.platform || null, status: filters.status || null,
    year: filters.year ? Number(filters.year) : null, min_rating: filters.rating ? Number(filters.rating) : null,
    regions: [],
    scope,
    genres,
    genre_mode: genres.length ? genreMode : null,
    cast,
    cast_mode: cast.length ? castMode : null,
    year_from: yearFrom ? Number(yearFrom) : null,
    year_to: yearTo ? Number(yearTo) : null,
    start: tab === 'daily' ? date : tab === 'weekly' ? range.start : null,
    end: tab === 'daily' ? shiftISO(date, 1) : tab === 'weekly' ? range.end : null,
    weekday: tab === 'weekly' ? weekday : null, sort, page, page_size: pageSize,
  }), [search, filters, tab, date, range.start, range.end, weekday, sort, page, pageSize,
      scope, genres, genreMode, cast, castMode, yearFrom, yearTo]);
  // 分面计数跟着筛选走，但翻页/排序不该触发重算；日期无效时先不算，等用户改对。
  const facetQuery = useMemo<CatalogQuery>(() => ({
    search: query.search, kind: query.kind, region: query.region,
    genre: query.genre, platform: query.platform, status: query.status,
    year: query.year, min_rating: query.min_rating,
    regions: [],
    scope: query.scope, genres: query.genres, genre_mode: query.genre_mode,
    cast: query.cast, cast_mode: query.cast_mode,
    year_from: query.year_from, year_to: query.year_to,
    start: dateValid ? query.start : null, end: dateValid ? query.end : null,
    weekday: query.weekday, sort: null, page: null, page_size: null,
  }), [query, dateValid]);
  const facetsQ = useQuery({ queryKey: ['catalog', 'facets', facetQuery], queryFn: () => queryCmd('catalog_facets', { query: facetQuery }) });
  const libraryQ = useQuery({ queryKey: ['catalog', 'library', query], queryFn: () => queryCmd('catalog_library', { query }), enabled: tab === 'library' });
  const releasesQ = useQuery({ queryKey: ['catalog', 'releases', query], queryFn: () => queryCmd('catalog_releases', { query }), enabled: isSchedule && dateValid });
  const overviewQ = useQuery({ queryKey: ['catalog', 'overview', scope], queryFn: () => queryCmd('catalog_overview', { scope }), enabled: tab === 'overview' });
  const columns = useMemo(catalogColumns, []);
  const scheduleColumns = useMemo(releaseColumns, []);
  const activeQ = tab === 'library' ? libraryQ : releasesQ;

  // SSE 只负责使本地查询失效；页面不发起首次采集，也不另建轮询调度。
  // 热刷新（catalog:updated）会把「最近更新」排到最前，列表自动跟着刷新。
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
  const changeScope = (next: Scope) => { setScope(next); setPage(1); };
  const addCast = () => {
    const name = castDraft.trim();
    if (!name || cast.includes(name)) return;
    setCast((old) => [...old, name]);
    setCastDraft('');
    setPage(1);
  };
  const changeTab = (next: Tab) => {
    setTab(next); setPage(1); setWeekday(null);
    if (next === 'weekly' && dateValid) setDate(mondayISO(date));
    if (next === 'daily' || next === 'weekly') setSort('time');
    else if (next === 'library' && sort === 'time') setSort('updated');
  };
  const resetFilters = () => {
    setSearch(''); setFilters(EMPTY_FILTERS); setGenres([]); setCast([]);
    setYearFrom(''); setYearTo(''); setWeekday(null); setPage(1);
  };
  const hasFilters = !!search || Object.values(filters).some(Boolean) || genres.length > 0
    || cast.length > 0 || !!yearFrom || !!yearTo || weekday !== null;

  return <div className="flex flex-col gap-3">
    <Card className="overflow-hidden p-0">
      <div className="flex flex-wrap items-center gap-2 border-b border-line bg-sunken p-2">
        {isScoped && <div className="flex gap-1" aria-label="资料库范围">
          {SCOPES.map((item) => <Button key={item.id} type="button" size="sm" variant={scope === item.id ? 'soft' : 'ghost'} aria-pressed={scope === item.id} onClick={() => changeScope(item.id)}>{item.label}</Button>)}
        </div>}
        <div className="flex flex-wrap gap-1" aria-label="媒体库视图">
          {TABS.map((item) => <Button key={item.id} type="button" size="sm" variant={tab === item.id ? 'soft' : 'ghost'} aria-pressed={tab === item.id} onClick={() => changeTab(item.id)}>{item.label}</Button>)}
        </div>
        <div className="flex min-w-0 flex-1 items-center gap-2 sm:min-w-48">
          <label htmlFor="catalog-search" className="shrink-0 text-xs font-medium text-fg2">搜索</label>
          <Tooltip content="概览与采集管理不接受作品筛选；输入搜索会转到媒体库">
            <Input id="catalog-search" type="search" aria-label="搜索作品名称或别名" placeholder="搜索作品名称、原名、别名" value={search} className="min-w-0 flex-1" onChange={(event) => { setSearch(event.target.value); setPage(1); if (!isList) changeTab('library'); }} />
          </Tooltip>
        </div>
        {isList && <div className="flex gap-1" aria-label="展示方式">
          <Button type="button" size="sm" variant={layout === 'grid' ? 'soft' : 'ghost'} aria-label="卡片视图" aria-pressed={layout === 'grid'} onClick={() => setLayout('grid')}><Grid2X2 size={13} aria-hidden /></Button>
          <Button type="button" size="sm" variant={layout === 'table' ? 'soft' : 'ghost'} aria-label="紧凑表格" aria-pressed={layout === 'table'} onClick={() => setLayout('table')}><List size={13} aria-hidden /></Button>
        </div>}
      </div>
      <div className="flex flex-col gap-2.5 p-2.5">
        <div className="flex flex-wrap items-center gap-2 rounded-md bg-sunken/40 px-2.5 py-2">
          <span className="shrink-0 text-xs font-medium text-fg2">类型</span>
          <div className="flex flex-wrap gap-1.5" aria-label="作品类型">
            <Tooltip content={!isList ? '类型筛选适用于媒体库与日历' : undefined}>
              <Button type="button" size="sm" variant={!filters.kind ? 'soft' : 'ghost'} disabled={!isList} aria-pressed={!filters.kind} onClick={() => changeFilter('kind', '')}>全部类型</Button>
            </Tooltip>
            {KIND_ORDER.map((kind) => {
              const facet = facetsQ.data?.kinds.find((item) => item.key === kind);
              return <Tooltip key={kind} content={!isList ? '类型筛选适用于媒体库与日历' : facet ? '计数来自当前资料库，不代表来源全量覆盖' : '该类型暂未返回数据库计数'}>
                <Button type="button" size="sm" variant={filters.kind === kind ? 'soft' : 'ghost'} disabled={!isList} aria-pressed={filters.kind === kind} onClick={() => changeFilter('kind', kind)}>{kindLabel(kind)}{facet ? `（${facet.count}）` : ''}</Button>
              </Tooltip>;
            })}
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2 rounded-md bg-sunken/40 px-2.5 py-2">
          <span className="shrink-0 text-xs font-medium text-fg2">题材</span>
          <div className="flex flex-wrap items-center gap-1.5">
            <SelectedTags items={genres} onRemove={(value) => { setGenres((old) => old.filter((item) => item !== value)); setPage(1); }} removeLabel={(value) => `移除题材 ${value}`} />
            <Picker label="添加题材" value="" size="sm" className="w-44" disabled={!isList || facetsQ.isLoading} onValueChange={(next) => {
              if (next && !genres.includes(next)) { setGenres((old) => [...old, next]); setPage(1); }
            }} options={[
              { value: '', label: genres.length ? '添加题材…' : '全部题材' },
              ...(facetsQ.data?.genres ?? []).filter((item) => !genres.includes(item.key)).map((item) => ({ value: item.key, label: `${item.label || item.key}（${item.count}）` })),
            ]} />
            <ModeToggle value={genreMode} onChange={(next) => { setGenreMode(next); setPage(1); }} label="题材" />
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2 rounded-md bg-sunken/40 px-2.5 py-2">
          <span className="shrink-0 text-xs font-medium text-fg2">演员</span>
          <div className="flex flex-wrap items-center gap-1.5">
            <SelectedTags items={cast} onRemove={(value) => { setCast((old) => old.filter((item) => item !== value)); setPage(1); }} removeLabel={(value) => `移除演员 ${value}`} />
            <Input aria-label="按演员名筛选，回车添加" placeholder="演员名，回车添加" value={castDraft} className="w-44" disabled={!isList}
              onChange={(event) => setCastDraft(event.target.value)}
              onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); addCast(); } }} />
            <Button type="button" size="sm" variant="ghost" disabled={!isList || !castDraft.trim()} onClick={addCast}>添加</Button>
            <ModeToggle value={castMode} onChange={(next) => { setCastMode(next); setPage(1); }} label="演员" />
          </div>
        </div>
        <div className="grid gap-2 rounded-md bg-sunken/40 p-2.5 sm:grid-cols-2 lg:grid-cols-4 2xl:grid-cols-8">
          <Field label="国家 / 地区" htmlFor="catalog-filter-region">
            <FacetFilter id="catalog-filter-region" label="国家 / 地区" value={filters.region} items={facetsQ.data?.regions ?? []} disabled={!isList || facetsQ.isLoading} onChange={(value) => changeFilter('region', value)} region />
          </Field>
          <Field label="平台" htmlFor="catalog-filter-platform">
            <FacetFilter id="catalog-filter-platform" label="平台" value={filters.platform} items={facetsQ.data?.platforms ?? []} disabled={!isList || facetsQ.isLoading} onChange={(value) => changeFilter('platform', value)} />
          </Field>
          <Field label="发行状态" htmlFor="catalog-filter-status">
            <Hint content={!isList ? '状态筛选适用于媒体库与日历' : '作品发行状态，不是观看状态'} focusable={false}>
              <Picker id="catalog-filter-status" label="发行状态" value={filters.status} disabled={!isList} size="sm" className="w-full max-w-none" onValueChange={(next) => changeFilter('status', next)} options={[
                { value: '', label: '全部发行状态' },
                ...Object.entries(MEDIA_STATUSES).map(([key, label]) => ({ value: key, label })),
              ]} />
            </Hint>
          </Field>
          <Field label="年份从" htmlFor="catalog-filter-year-from">
            <Hint content={!isList ? '年份区间适用于媒体库与日历' : '与「年份到」组成区间；只填一边就是单边下限'} focusable={false}>
              <Picker id="catalog-filter-year-from" label="年份从" value={yearFrom} disabled={!isList || facetsQ.isLoading} size="sm" className="w-full max-w-none" onValueChange={(next) => { setYearFrom(next); setPage(1); }} options={[
                { value: '', label: '不限' },
                ...(facetsQ.data?.years ?? []).map((year) => ({ value: String(year), label: `${year} 年` })),
              ]} />
            </Hint>
          </Field>
          <Field label="年份到" htmlFor="catalog-filter-year-to">
            <Picker id="catalog-filter-year-to" label="年份到" value={yearTo} disabled={!isList || facetsQ.isLoading} size="sm" className="w-full max-w-none" onValueChange={(next) => { setYearTo(next); setPage(1); }} options={[
              { value: '', label: '不限' },
              ...(facetsQ.data?.years ?? []).map((year) => ({ value: String(year), label: `${year} 年` })),
            ]} />
          </Field>
          <Field label="评分下限" htmlFor="catalog-filter-rating">
            <Hint content="后端选择一个用户/观众评分换算为十分制；来源见条目。不是专业评论或综合评分" focusable={false}>
              <Picker id="catalog-filter-rating" label="用户或观众评分下限，十分制" value={filters.rating} disabled={!isList} size="sm" className="w-full max-w-none" onValueChange={(next) => changeFilter('rating', next)} options={[
                { value: '', label: '用户/观众评分不限' },
                ...[6, 7, 8, 9].map((rating) => ({ value: String(rating), label: `用户/观众 ≥ ${rating} / 10` })),
              ]} />
            </Hint>
          </Field>
          <Field label="排序" htmlFor="catalog-filter-sort">
            {isList ? <Picker id="catalog-filter-sort" label="排序" value={sort} size="sm" className="w-full max-w-none" onValueChange={(next) => { setSort(next); setPage(1); }} options={[
              ...(isSchedule ? [{ value: 'time', label: '发行时间' }] : []),
              { value: 'updated', label: '最近更新' },
              { value: 'rating', label: '用户/观众评分' },
              { value: 'newest', label: isSchedule ? '近期发行优先' : '最新收录' },
              { value: 'title', label: '标题' },
              ...(!isSchedule ? [{ value: 'year', label: '年份' }] : []),
            ]} /> : <span className="text-xs text-muted">—</span>}
          </Field>
          {hasFilters && <div className="flex items-end"><Button type="button" size="sm" variant="ghost" onClick={resetFilters}><RotateCcw size={12} aria-hidden />清空筛选</Button></div>}
        </div>
        {isSchedule && <div className="flex flex-wrap items-center gap-2 rounded-md bg-sunken/40 px-2.5 py-2">
          <span className="shrink-0 text-xs font-medium text-fg2">发行日期</span>
          <Tooltip content={!dateValid ? '请先选择有效日期' : undefined}>
            <Button type="button" size="sm" variant="ghost" aria-label={tab === 'weekly' ? '上一天' : '上一天'} disabled={!dateValid} onClick={() => changeDate(shiftISO(date, tab === 'weekly' ? -7 : -1))}><ChevronLeft size={14} aria-hidden /></Button>
          </Tooltip>
          <Input type="date" aria-label={tab === 'weekly' ? '选择周一至周日的任一天' : '每日发行日期'} value={date} invalid={!dateValid} onChange={(event) => changeDate(event.target.value)} />
          <Tooltip content={!dateValid ? '请先选择有效日期' : undefined}>
            <Button type="button" size="sm" variant="ghost" aria-label={tab === 'weekly' ? '下一天' : '下一天'} disabled={!dateValid} onClick={() => changeDate(shiftISO(date, tab === 'weekly' ? 7 : 1))}><ChevronRight size={14} aria-hidden /></Button>
          </Tooltip>
          <Button type="button" size="sm" variant="ghost" onClick={() => changeDate(todayISO())}>{tab === 'weekly' ? '本周' : '今天'}</Button>
          {dateValid && <span className="text-xs tabular-nums text-fg2">{tab === 'weekly' ? `${formatDay(range.start)} — ${formatDay(range.last)}` : formatDay(date)}</span>}
          {tab === 'weekly' && <div className="flex flex-wrap items-center gap-1">
            <span className="shrink-0 text-xs font-medium text-fg2">发行星期</span>
            <div className="flex flex-wrap gap-1" aria-label="星期筛选">
              <Button type="button" size="sm" variant={weekday === null ? 'soft' : 'ghost'} aria-pressed={weekday === null} onClick={() => { setWeekday(null); setPage(1); }}>全周</Button>
              {WEEKDAYS.map((label, index) => <Button key={label} type="button" size="sm" variant={weekday === index + 1 ? 'soft' : 'ghost'} aria-pressed={weekday === index + 1} aria-label={`只看周${label}`} onClick={() => { setWeekday(index + 1); setPage(1); }}>周{label}</Button>)}
            </div>
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
    {tab === 'follow' && <CatalogFollowView onOpen={setMediaKey} />}
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
    <Picker label="每页条数" value={String(pageSize)} onValueChange={(next) => onPageSize(Number(next))} options={[24, 48, 100].map((size) => ({ value: String(size), label: `${size} 条 / 页` }))} />
    <Tooltip content={page <= 1 ? '已是第一页' : loading ? '正在查询' : undefined}>
      <Button type="button" size="sm" variant="ghost" disabled={loading || page <= 1} aria-label="上一页" onClick={() => onPage(page - 1)}><ChevronLeft size={13} aria-hidden />上一页</Button>
    </Tooltip>
    <Tooltip content={page >= pages ? '已是最后一页' : loading ? '正在查询' : undefined}>
      <Button type="button" size="sm" variant="ghost" disabled={loading || page >= pages} aria-label="下一页" onClick={() => onPage(page + 1)}>下一页<ChevronRight size={13} aria-hidden /></Button>
    </Tooltip>
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
