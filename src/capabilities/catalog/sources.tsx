import { useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { KeyRound, RefreshCw, RotateCcw } from 'lucide-react';
import { Button, Card, Dialog, Field, Input, Notice, Picker, Section, SkeletonBlock, StateBlock, Tag, Tooltip, confirmAction } from '@/components/ui';
import { DataTable, type Column } from '@/components/ui/table';
import { errMsg, queryCmd, runCmd, useBusy } from '@/lib/cmd';
import type { CatalogJob, CatalogRefreshReport, CatalogSourceStatus, CatalogSyncRequest } from '@/lib/types';
import { SYNC_MODES, canRetry, jobStatusLabel, jobTone, sourceLabel, syncDateError, textOrUnknown, type SyncMode } from './helpers';

type Counters = Pick<CatalogJob, 'fetched' | 'added' | 'updated' | 'deduplicated' | 'releases' | 'failed' | 'pages' | 'pagination_complete' | 'cursor' | 'http_status'>;
function Counts({ row }: { row: Counters }) {
  return <div className="space-y-1 text-xs tabular-nums"><div>拉取 {row.fetched} · 新增 {row.added} · 更新 {row.updated}</div><div>去重 {row.deduplicated} · 发行 {row.releases} · 失败 {row.failed}</div></div>;
}
function Progress({ row, attempted = true }: { row: Counters; attempted?: boolean }) {
  return <div className="space-y-1 text-xs"><div>{row.pages} 页 · {attempted ? row.pagination_complete ? '分页完成' : '分页未完成' : '尚未采集'}</div><div>HTTP {row.http_status ?? '暂无资料'}</div><div className="break-all">游标 {textOrUnknown(row.cursor)}</div></div>;
}
function Problems({ row }: { row: Pick<CatalogJob, 'error' | 'warning'> }) {
  return <div className="space-y-1 text-xs">{row.error && <Notice tone="bad">{row.error}</Notice>}{row.warning && <Notice tone="warn">{row.warning}</Notice>}{!row.error && !row.warning && <span className="text-muted">无已报告问题</span>}</div>;
}
function sourceReady(row: CatalogSourceStatus): boolean {
  return row.implemented && row.enabled && (!row.requires_key || row.configured) && row.status !== 'running';
}
function unavailableReason(row: CatalogSourceStatus): string {
  if (!row.implemented) return '该来源未接入';
  if (!row.enabled) return '请先启用来源';
  if (row.requires_key && !row.configured) return '请先配置授权密钥';
  return row.status === 'running' ? '该来源正在采集' : '';
}

export function CatalogSources() {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const [mode, setMode] = useState<SyncMode>('incremental');
  const [target, setTarget] = useState('');
  const [start, setStart] = useState('');
  const [end, setEnd] = useState('');
  const [report, setReport] = useState<CatalogRefreshReport | null>(null);
  const [keySource, setKeySource] = useState<CatalogSourceStatus | null>(null);
  const [token, setToken] = useState('');
  const [keyError, setKeyError] = useState<unknown>(null);
  const statusQ = useQuery({ queryKey: ['catalog', 'status'], queryFn: () => queryCmd('catalog_status') });
  const jobsQ = useQuery({ queryKey: ['catalog', 'jobs'], queryFn: () => queryCmd('catalog_jobs') });
  const sources = statusQ.data ?? [];
  const dateError = syncDateError(mode, start, end);
  const available = target ? sources.filter((row) => row.source === target && sourceReady(row)) : sources.filter(sourceReady);
  const invalidate = () => qc.invalidateQueries({ queryKey: ['catalog'] });

  const refresh = (source?: string) => run(`refresh:${source ?? 'all'}`, async () => {
    if (dateError) return;
    const request: CatalogSyncRequest = { sources: source ? [source] : target ? [target] : null, mode, start: start || null, end: end || null };
    const result = await runCmd('catalog_refresh', { request });
    if (result !== undefined) { setReport(result); await invalidate(); }
  });
  const toggle = (row: CatalogSourceStatus) => run(`toggle:${row.source}`, async () => {
    const result = await runCmd('catalog_source_set', { source: row.source, enabled: !row.enabled });
    if (result !== undefined) { qc.setQueryData(['catalog', 'status'], result); await invalidate(); }
  });
  const retry = (row: CatalogJob) => run(`retry:${row.id}`, async () => {
    const result = await runCmd('catalog_retry', { id: row.id });
    if (result !== undefined) { setReport(result); await invalidate(); }
  });
  const closeKey = () => { setKeySource(null); setToken(''); setKeyError(null); };
  const saveKey = () => run('key:save', async () => {
    if (!keySource || !token.trim()) return;
    const enabled = sources.find((row) => row.source === keySource.source)?.enabled ?? keySource.enabled;
    setKeyError(null);
    // token 是命令封装层识别的脱敏参数名，不改成 key/value 或嵌套对象。
    const result = await runCmd('catalog_source_set', { source: keySource.source, enabled, token: token.trim() }, { onError: setKeyError });
    setToken('');
    if (result !== undefined) { qc.setQueryData(['catalog', 'status'], result); closeKey(); await invalidate(); }
  });
  const clearKey = (row: CatalogSourceStatus) => run('key:clear', async () => {
    if (!(await confirmAction({ title: '清除来源密钥', message: `清除 ${row.label} 的本机授权密钥？该来源将无法继续调用需要授权的接口。`, danger: true, okText: '清除密钥' }))) return;
    const result = await runCmd('catalog_source_set', { source: row.source, enabled: row.enabled, clear_key: true });
    if (result !== undefined) { qc.setQueryData(['catalog', 'status'], result); closeKey(); await invalidate(); }
  });

  const columns: Column<CatalogSourceStatus>[] = [
    { key: 'source', header: '来源 / 收录量', wrap: true, cell: (row) => <div className="space-y-1"><div className="font-medium">{row.label}</div><div className="text-xs text-muted">{row.source} · {row.item_count} 条</div><Tag tone={row.implemented ? 'info' : 'muted'}>{row.implemented ? '已接入' : '未接入'}</Tag></div> },
    { key: 'config', header: '配置 / 启用', wrap: true, cell: (row) => <div className="space-y-1 text-xs"><div>{row.requires_key ? row.configured ? '密钥已配置' : '密钥未配置' : '无需密钥'}</div><div>{row.enabled ? '已启用' : '已停用'}</div><Tag tone={!row.implemented ? 'muted' : jobTone(row.status)}>{row.implemented ? jobStatusLabel(row.status) : '未连接'}</Tag></div> },
    { key: 'coverage', header: '覆盖范围 / 许可', wrap: true, cell: (row) => <div className="min-w-48 space-y-1 text-xs"><div>{textOrUnknown(row.coverage)}</div><div className="text-muted">{textOrUnknown(row.license_note)}</div></div> },
    { key: 'time', header: '最后尝试 / 成功', wrap: true, cell: (row) => <div className="space-y-1 text-xs"><div>尝试 {row.last_attempt ?? '尚未尝试'}</div><div>成功 {row.last_success ?? '尚未成功'}</div></div> },
    { key: 'counts', header: '上次采集结果', wrap: true, cell: (row) => <Counts row={row} /> },
    { key: 'progress', header: '分页 / HTTP / 游标', wrap: true, cell: (row) => <Progress row={row} attempted={row.last_attempt !== null} /> },
    { key: 'error', header: '错误 / 警告', wrap: true, cell: (row) => <Problems row={row} /> },
    { key: 'actions', header: '来源操作', wrap: true, cell: (row) => <div className="flex flex-wrap gap-1">
      <Tooltip content={dateError || unavailableReason(row) || '按上方采集模式与日期范围刷新此来源'}>
        <Button type="button" size="sm" variant="soft" disabled={!!busy || !sourceReady(row) || !!dateError} loading={busy === `refresh:${row.source}`} onClick={() => void refresh(row.source)}><RefreshCw size={12} aria-hidden />采集</Button>
      </Tooltip>
      <Tooltip content={!row.implemented ? '该来源尚未接入' : row.status === 'running' ? '采集完成后可修改' : undefined}>
        <Button type="button" size="sm" variant="ghost" disabled={!!busy || !row.implemented || row.status === 'running'} loading={busy === `toggle:${row.source}`} aria-label={`${row.enabled ? '停用' : '启用'} ${row.label}`} aria-pressed={row.enabled} onClick={() => void toggle(row)}>{row.enabled ? '停用' : '启用'}</Button>
      </Tooltip>
      {row.requires_key && row.implemented && <Tooltip content={row.status === 'running' ? '采集完成后可修改密钥' : undefined}>
        <Button type="button" size="sm" variant="ghost" disabled={!!busy || row.status === 'running'} aria-label={`配置 ${row.label} 密钥`} onClick={() => { setToken(''); setKeyError(null); setKeySource(row); }}><KeyRound size={12} aria-hidden />密钥</Button>
      </Tooltip>}
    </div> },
  ];
  const jobColumns: Column<CatalogJob>[] = [
    { key: 'job', header: '任务 / 来源 / 模式', wrap: true, cell: (row) => <div className="space-y-1 text-xs"><div>{sourceLabel(row.source)} · {SYNC_MODES[row.mode as SyncMode] ?? row.mode}</div><div className="break-all text-muted">{row.id}</div></div> },
    { key: 'status', header: '状态 / 时间', wrap: true, cell: (row) => <div className="space-y-1"><Tag tone={jobTone(row.status)}>{jobStatusLabel(row.status)}</Tag><div className="text-xs">开始 {textOrUnknown(row.started_at)}</div><div className="text-xs">结束 {row.finished_at ?? '尚未结束'}</div></div> },
    { key: 'counts', header: '采集结果', wrap: true, cell: (row) => <Counts row={row} /> },
    { key: 'progress', header: '分页 / HTTP / 游标', wrap: true, cell: (row) => <Progress row={row} /> },
    { key: 'error', header: '错误 / 警告', wrap: true, cell: (row) => <Problems row={row} /> },
    { key: 'retry', header: '操作', cell: (row) => {
      const source = sources.find((item) => item.source === row.source);
      return canRetry(row.status) ? <Tooltip content={source ? unavailableReason(source) || '重试此失败/部分完成任务' : '来源状态尚未读取'}>
        <Button type="button" size="sm" variant="soft" disabled={!!busy || !source || !sourceReady(source)} loading={busy === `retry:${row.id}`} onClick={() => void retry(row)}><RotateCcw size={12} aria-hidden />重试</Button>
      </Tooltip> : <span className="text-xs text-muted">无重试操作</span>;
    } },
  ];

  return <div className="flex flex-col gap-3">
    <Card className="flex flex-col gap-3 bg-sunken p-3">
      <div className="flex flex-wrap items-end gap-3">
        <Field label="采集模式" htmlFor="catalog-sync-mode"><Picker id="catalog-sync-mode" value={mode} disabled={!!busy} onValueChange={(next) => setMode(next as SyncMode)} options={Object.entries(SYNC_MODES).map(([key, label]) => ({ value: key, label }))} /></Field>
        <Field label="来源" htmlFor="catalog-sync-target"><Picker id="catalog-sync-target" value={target} disabled={!!busy} onValueChange={setTarget} options={[
          { value: '', label: '全部已启用且已配置来源' },
          ...sources.filter((row) => row.implemented).map((row) => ({
            value: row.source,
            label: `${row.label}${!sourceReady(row) ? `（${unavailableReason(row)}）` : ''}`,
            disabled: !sourceReady(row),
          })),
        ]} /></Field>
        <Field label="开始日期" htmlFor="catalog-sync-start"><Tooltip content="不填则使用后端配置范围；历史回填必须填写">
          <Input id="catalog-sync-start" type="date" value={start} disabled={!!busy} invalid={!!dateError && !!start} onChange={(event) => setStart(event.target.value)} />
        </Tooltip></Field>
        <Field label="结束日期（不含当天）" htmlFor="catalog-sync-end"><Tooltip content="开始日期包含，结束日期不包含；历史回填必须填写">
          <Input id="catalog-sync-end" type="date" value={end} disabled={!!busy} invalid={!!dateError && !!end} onChange={(event) => setEnd(event.target.value)} />
        </Tooltip></Field>
        <Tooltip content={dateError || (!available.length ? '没有可采集的已接入、已启用且已配置来源' : '手动采集；自动调度由后端负责')}>
          <Button type="button" size="sm" variant="primary" disabled={!!busy || !!dateError || !available.length || statusQ.isError || statusQ.isLoading} loading={busy === 'refresh:all'} onClick={() => void refresh()}><RefreshCw size={13} aria-hidden />手动采集</Button>
        </Tooltip>
      </div>
      {dateError && <Notice tone="warn">{dateError}</Notice>}
      {report && <div className="text-xs text-fg2" role="status">上次手动任务：新增 {report.added} · 更新 {report.updated} · 发行 {report.releases} · 全库 {report.total}；逐源结果见下表。</div>}
    </Card>
    {statusQ.isError ? <StateBlock kind="error" title="来源状态读取失败" detail={errMsg(statusQ.error)} onRetry={() => void statusQ.refetch()} /> : statusQ.isLoading ? <SkeletonBlock rows={6} /> : <div className="overflow-x-auto rounded-lg border border-line bg-card"><DataTable columns={columns} rows={sources} rowKey={(row) => row.source} dense empty={<StateBlock kind="empty" title="暂无已登记来源" />} /></div>}
    <Section title="采集任务记录" flush>
      {jobsQ.isError ? <StateBlock kind="error" title="采集记录读取失败" detail={errMsg(jobsQ.error)} onRetry={() => void jobsQ.refetch()} /> : jobsQ.isLoading ? <SkeletonBlock rows={5} className="p-3" /> : <div className="overflow-x-auto"><DataTable columns={jobColumns} rows={jobsQ.data ?? []} rowKey={(row) => row.id} dense empty={<StateBlock kind="empty" title="暂无采集任务" detail="页面不会在首次进入时触发采集；可手动采集或等待后端调度。" />} /></div>}
    </Section>
    <Dialog open={keySource !== null} title={`${keySource?.label ?? ''} 授权密钥`} subtitle="只保存到本机；已保存凭据不回显。" onClose={closeKey} footer={<>
      {keySource?.configured && <Button type="button" size="sm" variant="danger" disabled={!!busy} loading={busy === 'key:clear'} onClick={() => void clearKey(keySource)}>清除密钥</Button>}
      <Button type="button" size="sm" variant="ghost" onClick={closeKey}>取消</Button>
      <Button type="submit" form="catalog-key-form" size="sm" variant="primary" disabled={!!busy || !token.trim()} loading={busy === 'key:save'}>保存密钥</Button>
    </>}>
      <form id="catalog-key-form" onSubmit={(event) => { event.preventDefault(); void saveKey(); }} className="space-y-3">
        <Field label="授权 Token" htmlFor="catalog-source-token" hint={keySource?.configured ? '已配置；输入新密钥会替换原密钥' : '未配置'}><Input id="catalog-source-token" type="password" value={token} autoComplete="new-password" spellCheck={false} disabled={!!busy} className="w-full" onChange={(event) => setToken(event.target.value)} /></Field>
        {keySource?.license_note && <Notice>{keySource.license_note}</Notice>}
        {keyError !== null && <Notice tone="bad">{errMsg(keyError)}</Notice>}
      </form>
    </Dialog>
  </div>;
}
