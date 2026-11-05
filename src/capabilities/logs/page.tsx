// 日志：查询 / 导出 / 实时跟随。原先是设置页里的一个 tab，独立成一级能力。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { queryCmd, runCmd } from '@/lib/cmd';
import { useEffect, useRef, useState } from 'react';
import { errText } from '@/lib/ipc';
import { Button } from '@/components/ui/button';
import { cn } from '@/components/ui/button';
import { Checkbox, Input, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { fmtMs } from '@/lib/format';
import type { LogFilter, LogLine } from '@/lib/types';

export default function LogsPage() {
  return <LogsPane />;
}

function LogsPane() {
  const [filter, setFilter] = useState<LogFilter>({ level: null, module: null, keyword: null, limit: 300 });
  // 只看异常：排障第一步。级别筛选是「精确等于」，没法表达 warn+error，
  // 所以在客户端做一次 ≥ warn 的过滤。
  const [failOnly, setFailOnly] = useState(false);
  const [exporting, setExporting] = useState(false);
  const query = useQuery<LogLine[]>({
    // 「只看异常」时多读一些：失败是稀疏的，300 行里可能一条都没有，
    // 用户会误以为「没出过错」
    queryKey: ['log', JSON.stringify(filter), failOnly],
    queryFn: () =>
      queryCmd('log_query', {
        filter: failOnly ? { ...filter, limit: 1000 } : filter,
      }),
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
  const rows = [...(query.data ?? []), ...liveRows].filter(
    (line) => !failOnly || line.level === 'warn' || line.level === 'error',
  );

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
          <label
            className="flex cursor-pointer items-center gap-1 text-xs text-fg2"
            title="只看 warn / error：排障时先看这里"
          >
            <Checkbox checked={failOnly} onChange={(e) => setFailOnly(e.target.checked)} />
            只看异常
          </label>
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
            <LogRow key={i} line={line} />
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

/** RFC3339 转 MM-DD HH:mm:ss.SSS：同一秒内的连续步骤靠毫秒才分得清先后。 */
function shortTs(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const p = (n: number) => String(n).padStart(2, '0');
  return (
    p(d.getMonth() + 1) +
    '-' +
    p(d.getDate()) +
    ' ' +
    p(d.getHours()) +
    ':' +
    p(d.getMinutes()) +
    ':' +
    p(d.getSeconds()) +
    '.' +
    String(d.getMilliseconds()).padStart(3, '0')
  );
}

/** 实时行走与后端查询同一套过滤语义（级别精确、模块/关键词包含）。 */
function matchFilter(line: LogLine, filter: LogFilter): boolean {
  if (filter.level && line.level !== filter.level) return false;
  if (filter.module && !line.module.includes(filter.module)) return false;
  if (filter.keyword) {
    // 与后端同口径：报错原文常常只在证据里，只搜 message 会「明明记了却搜不到」
    const haystack = [
      line.message,
      line.subject,
      line.step,
      line.evidence,
      line.action,
      line.batch,
    ]
      .filter(Boolean)
      .join(' ');
    if (!haystack.includes(filter.keyword)) return false;
  }
  return true;
}

const TRIGGER_LABEL: Record<string, string> = {
  user: '手动',
  schedule: '定时',
  startup: '启动',
};

/** 结果分档：跳过不是失败，混在一起就判断不了要不要人工介入。 */
const OUTCOME: Record<string, { text: string; className: string }> = {
  ok: { text: '成功', className: 'text-ok' },
  skip: { text: '跳过', className: 'text-muted' },
  fail: { text: '失败', className: 'text-bad' },
  partial: { text: '部分成功', className: 'text-warn' },
};

/**
 * 一行日志：抬头（时间/级别/主体/动作/结果/耗时）+ 正文 + 结构化字段。
 *
 * 结构化字段单独一行而不是塞进正文：`环节` 和 `证据` 是排障时唯一有用的部分，
 * 挤在长句里会被截断、也没法一眼扫到。
 */
function LogRow({ line }: { line: LogLine }) {
  const outcome = line.outcome ? OUTCOME[line.outcome] : null;
  const failed = line.level === 'error' || line.level === 'warn';
  return (
    <div
      className={cn(
        'border-b border-line px-2 py-1 text-xs',
        line.level === 'error' ? 'bg-bad/5' : line.level === 'warn' ? 'bg-warn/5' : '',
      )}
    >
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 font-mono">
        <span className="text-muted tabular-nums">{shortTs(line.ts)}</span>
        <span
          className={cn(
            'rounded px-1 text-[10px] font-semibold uppercase',
            line.level === 'error'
              ? 'bg-bad/15 text-bad'
              : line.level === 'warn'
                ? 'bg-warn/15 text-warn'
                : 'bg-hover text-muted',
          )}
        >
          {line.level}
        </span>
        {line.subject && (
          <span className="font-semibold text-fg" title={line.subject}>
            {line.subject}
          </span>
        )}
        <span className="text-fg2">{line.action ?? line.module}</span>
        {line.trigger && (
          <span className="text-muted">[{TRIGGER_LABEL[line.trigger] ?? line.trigger}]</span>
        )}
        {outcome && <span className={cn('font-medium', outcome.className)}>{outcome.text}</span>}
        {line.ms !== null && line.ms !== undefined && (
          <span className="text-muted tabular-nums">{fmtMs(line.ms)}</span>
        )}
        {line.batch && <span className="text-muted">批次={line.batch}</span>}
      </div>
      <div
        className={cn(
          'mt-0.5 whitespace-pre-wrap break-words pl-1',
          failed ? 'text-fg' : 'text-fg2',
        )}
      >
        {line.message}
      </div>
      {(line.step || line.evidence || line.attempts) && (
        <div className="mt-0.5 flex flex-wrap gap-x-3 gap-y-0.5 pl-1 text-[11px]">
          {line.step && <span className="text-muted">环节={line.step}</span>}
          {line.evidence && (
            <span className="break-all text-muted" title={line.evidence}>
              证据={line.evidence}
            </span>
          )}
          {line.attempts && <span className="text-muted">重试={line.attempts}</span>}
        </div>
      )}
    </div>
  );
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
