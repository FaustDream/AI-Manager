// 日志：查询 / 导出 / 实时跟随。原先是设置页里的一个 tab，独立成一级能力。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { queryCmd, runCmd } from '@/lib/cmd';
import { useEffect, useRef, useState } from 'react';
import { errText } from '@/lib/ipc';
import { Button } from '@/components/ui/button';
import { Input, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import type { LogFilter, LogLine } from '@/lib/types';

export default function LogsPage() {
  return <LogsPane />;
}

function LogsPane() {
  const [filter, setFilter] = useState<LogFilter>({ level: null, module: null, keyword: null, limit: 300 });
  const [exporting, setExporting] = useState(false);
  const query = useQuery<LogLine[]>({
    queryKey: ['log', JSON.stringify(filter)],
    queryFn: () => queryCmd('log_query', { filter }),
  });
  const live = useQuery<LogLine[]>({
    queryKey: ['log-live'],
    queryFn: () => [],
    staleTime: Infinity,
  });
  const tailRef = useRef<HTMLDivElement | null>(null);
  // 贴底才自动跟随：用户向上翻历史时，新日志不许把视口拽走
  const nearBottom = useRef(true);

  const liveRows = (live.data ?? []).filter((line) => matchFilter(line, filter));
  const rows = [...(query.data ?? []), ...liveRows];

  useEffect(() => {
    if (nearBottom.current) {
      tailRef.current?.scrollIntoView({ block: 'end' });
    }
  }, [rows.length]);

  return (
    <div className="flex h-[calc(100vh-7.5rem)] flex-col gap-2">
      {/* 筛选吸顶在滚动容器内部：翻日志时筛选条不跟着滚走 */}
      <div className="min-h-0 flex-1 overflow-auto rounded-lg border border-line bg-card">
        <div className="sticky top-0 z-10 flex flex-wrap items-center gap-2 border-b border-line bg-bg px-2 py-1.5">
          <Select
            value={filter.level ?? ''}
            onChange={(e) => setFilter((f) => ({ ...f, level: e.target.value || null }))}
          >
            <option value="">全部级别</option>
            <option value="info">info</option>
            <option value="warn">warn</option>
            <option value="error">error</option>
          </Select>
          <Input
            placeholder="模块"
            className="w-28"
            value={filter.module ?? ''}
            onChange={(e) => setFilter((f) => ({ ...f, module: e.target.value || null }))}
          />
          <Input
            placeholder="关键词"
            className="w-32"
            value={filter.keyword ?? ''}
            onChange={(e) => setFilter((f) => ({ ...f, keyword: e.target.value || null }))}
          />
          <Select
            value={filter.limit ? String(filter.limit) : '300'}
            onChange={(e) => setFilter((f) => ({ ...f, limit: Number(e.target.value) || null }))}
          >
            <option value="100">最近 100 行</option>
            <option value="300">最近 300 行</option>
            <option value="1000">最近 1000 行</option>
          </Select>
          <span className="flex-1" />
          <LogLevelPicker />
          <Button
            size="sm"
            loading={exporting}
            onClick={async () => {
              setExporting(true);
              try {
                toast(await queryCmd('log_export', { path: null, filter }));
              } catch (e) {
                toast(errText(e));
              } finally {
                setExporting(false);
              }
            }}
          >
            导出
          </Button>
        </div>
        <div className="p-2">
          {query.isLoading && <SkeletonRows rows={8} cols={1} />}
          {query.isError && (
            <StateBlock kind="error" title="读日志失败" detail={errText(query.error)} onRetry={() => query.refetch()} />
          )}
          {!query.isLoading && rows.length === 0 && (
            <p className="py-6 text-center text-sm text-muted">没有符合条件的日志。</p>
          )}
          {rows.map((line, i) => (
            <div
              key={i}
              className={`whitespace-pre-wrap break-words border-b border-line px-2 py-0.5 font-mono text-xs ${
                line.level === 'error' ? 'text-bad' : line.level === 'warn' ? 'text-warn' : ''
              }`}
            >
              <span className="text-muted">{shortTs(line.ts)}</span>{' '}
              <span className="font-semibold">[{line.level}]</span> {line.module} · {line.message}
              {line.task ? ` · ${line.task}` : ''}
            </div>
          ))}
          <div ref={tailRef} />
        </div>
      </div>
      <div className="flex items-center justify-between text-xs text-muted">
        <span>显示 {rows.length} 行{liveRows.length > 0 ? `（实时 ${liveRows.length}）` : ''}</span>
        <button
          className="cursor-pointer hover:text-fg"
          onClick={() => {
            nearBottom.current = true;
            tailRef.current?.scrollIntoView({ block: 'end' });
          }}
        >
          回到底部
        </button>
      </div>
    </div>
  );
}

/** RFC3339 转 MM-DD HH:mm:ss：日志行的时间要短，一眼对得上。 */
function shortTs(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const p = (n: number) => String(n).padStart(2, '0');
  return p(d.getMonth() + 1) + '-' + p(d.getDate()) + ' ' + p(d.getHours()) + ':' + p(d.getMinutes()) + ':' + p(d.getSeconds());
}

/** 实时行走与后端查询同一套过滤语义（级别精确、模块/关键词包含）。 */
function matchFilter(line: LogLine, filter: LogFilter): boolean {
  if (filter.level && line.level !== filter.level) return false;
  if (filter.module && !line.module.includes(filter.module)) return false;
  if (filter.keyword && !line.message.includes(filter.keyword)) return false;
  return true;
}

function LogLevelPicker() {
  const qc = useQueryClient();
  const [level, setLevel] = useState<string | null>(null);
  useEffect(() => {
    queryCmd('settings_get', { key: 'log.level' })
      .then((v) => setLevel((v as string) || 'info'))
      .catch(() => {});
  }, []);
  return (
    <Select
      value={level ?? 'info'}
      title="运行日志级别"
      onChange={async (e) => {
        const value = e.target.value;
        setLevel(value);
        
          if ((await runCmd('log_level', { level: value })) === undefined) return;
          await queryCmd('settings_set', { key: 'log.level', value });
          toast(`日志级别 → ${value}`);
          qc.invalidateQueries({ queryKey: ['log'] });
        
      }}
    >
      <option value="debug">debug</option>
      <option value="info">info</option>
      <option value="warn">warn</option>
      <option value="error">error</option>
    </Select>
  );
}
