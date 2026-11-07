// 日志：全部日志 / 执行动作双视图、增强筛选、实时跟随与导出。
// 执行动作不再单独成页——两者本来就是同一份 app.jsonl，只是看的角度不同。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useRef, useState } from 'react';
import { queryCmd, runCmd } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { Button, cn } from '@/components/ui/button';
import { confirmAction } from '@/components/ui/confirm';
import { Input, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { fmtMs } from '@/lib/format';
import type { Level, LogFilter, LogLine, LogStat } from '@/lib/types';

type View = 'all' | 'action' | 'stat';

const LEVEL_RANK: Record<Level, number> = { debug: 0, info: 1, warn: 2, error: 3 };

const TRIGGER_LABEL: Record<string, string> = {
  user: '手动',
  auto: '自动',
  schedule: '定时',
  startup: '启动',
  tray: '托盘',
};

const OUTCOME: Record<string, { text: string; className: string }> = {
  ok: { text: '成功', className: 'text-ok' },
  skip: { text: '跳过', className: 'text-muted' },
  fail: { text: '失败', className: 'text-bad' },
  partial: { text: '部分成功', className: 'text-warn' },
};

export default function LogsPage() {
  return <LogsPane />;
}

function LogsPane() {
  const qc = useQueryClient();
  const [view, setView] = useState<View>('all');
  const [minLevel, setMinLevel] = useState<Level | null>(null);
  const [module, setModule] = useState('');
  const [keyword, setKeyword] = useState('');
  const [outcome, setOutcome] = useState('');
  const [trigger, setTrigger] = useState('');
  const [subject, setSubject] = useState('');
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [limit, setLimit] = useState(300);
  const [exporting, setExporting] = useState(false);
  const [clearing, setClearing] = useState(false);

  const filter: LogFilter = {
    level: null,
    // 「只看异常」用最低级别表达，一次覆盖 warn + error
    min_level: minLevel,
    module: module || null,
    keyword: keyword || null,
    subject: subject || null,
    action_only: view === 'action' ? true : null,
    outcome: view === 'action' ? outcome || null : null,
    trigger: view === 'action' ? trigger || null : null,
    from: startIso(from),
    to: endIso(to),
    hide_legacy: view === 'action' ? true : null,
    limit,
  };

  const query = useQuery<LogLine[]>({
    queryKey: ['log', JSON.stringify(filter)],
    queryFn: () => queryCmd('log_query', { filter }),
  });
  const groups = useQuery<LogStat[]>({
    queryKey: ['log-stats', JSON.stringify(filter)],
    queryFn: () => queryCmd('log_stats', { filter }),
    enabled: view === 'stat',
  });
  const live = useQuery<LogLine[]>({
    queryKey: ['log-live'],
    queryFn: () => [],
    staleTime: Infinity,
  });
  const tailRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  // 贴底才自动跟随：用户向上翻历史时，新日志不许把视口拽走
  const nearBottom = useRef(true);

  const rows = useMemo(() => {
    const liveRows = (live.data ?? []).filter((line) => matchFilter(line, filter));
    const seen = new Set<string>();
    return [...(query.data ?? []), ...liveRows]
      .filter((line) => {
        // repeat 也要进 key：首行与合并行内容相同，只差次数
        const key = `${line.ts}|${line.module}|${line.action ?? ''}|${line.repeat ?? ''}|${line.message}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      })
      .sort((a, b) => a.ts.localeCompare(b.ts));
  }, [query.data, live.data, JSON.stringify(filter)]);

  const stats = useMemo(() => {
    // 合并行代表 N 次真实发生：统计要按次数算，否则「异常 3」会漏掉被合并掉的 97 次
    const weight = (line: LogLine) => line.repeat ?? 1;
    const sum = (pick: (line: LogLine) => boolean) =>
      rows.filter(pick).reduce((n, line) => n + weight(line), 0);
    return {
      total: rows.length,
      abnormal: sum((line) => line.level === 'warn' || line.level === 'error'),
      failed: sum((line) => effectiveOutcome(line) === 'fail'),
      skipped: sum((line) => effectiveOutcome(line) === 'skip'),
    };
  }, [rows]);

  useEffect(() => {
    if (nearBottom.current) {
      tailRef.current?.scrollIntoView({ block: 'end' });
    }
  }, [rows.length]);

  return (
    <div className="flex h-[calc(100vh-7.5rem)] flex-col gap-2">
      <div
        ref={listRef}
        className="min-h-0 flex-1 overflow-auto rounded-lg border border-line bg-card"
        onScroll={() => {
          const el = listRef.current;
          if (el) nearBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        <div className="sticky top-0 z-10 border-b border-line bg-bg">
          <div className="flex flex-wrap items-center gap-2 px-2 py-1.5">
            <div className="flex items-center gap-1 rounded-md border border-line p-0.5">
              <Button
                size="sm"
                variant={view === 'all' ? 'primary' : 'ghost'}
                onClick={() => {
                  setView('all');
                  setOutcome('');
                  setTrigger('');
                  setSubject('');
                  setFrom('');
                  setTo('');
                }}
              >
                全部日志
              </Button>
              <Button
                size="sm"
                variant={view === 'action' ? 'primary' : 'ghost'}
                onClick={() => setView('action')}
              >
                执行动作
              </Button>
              <Button
                size="sm"
                variant={view === 'stat' ? 'primary' : 'ghost'}
                onClick={() => setView('stat')}
                title="按「模块 + 动作 + 结果」聚合，回答哪一类在反复出问题"
              >
                聚合
              </Button>
            </div>
            <Select
              value={minLevel ?? ''}
              title="只看该级别及以上"
              onChange={(e) => setMinLevel((e.target.value || null) as Level | null)}
            >
              <option value="">全部级别</option>
              <option value="debug">debug+</option>
              <option value="info">info+</option>
              <option value="warn">warn+</option>
              <option value="error">error</option>
            </Select>
            <Input
              placeholder="模块"
              className="w-24"
              value={module}
              onChange={(e) => setModule(e.target.value)}
            />
            <Input
              placeholder="关键词 / 证据"
              className="w-36"
              value={keyword}
              onChange={(e) => setKeyword(e.target.value)}
            />
            <Select
              value={String(limit)}
              onChange={(e) => setLimit(Number(e.target.value) || 300)}
            >
              <option value="100">最近 100 行</option>
              <option value="300">最近 300 行</option>
              <option value="1000">最近 1000 行</option>
            </Select>
            <span className="flex-1" />
            <Button
              size="sm"
              loading={clearing}
              onClick={async () => {
                const ok = await confirmAction({
                  title: '清空日志',
                  message: '清空当前 app.log / app.jsonl 与历史轮转文件？\n只删日志，不动账号、会话与积分数据。',
                  danger: true,
                });
                if (!ok) return;
                setClearing(true);
                try {
                  if ((await runCmd('log_clear')) === undefined) return;
                  qc.setQueryData(['log-live'], []);
                  await qc.invalidateQueries({ queryKey: ['log'] });
                  toast('日志已清空');
                } finally {
                  setClearing(false);
                }
              }}
            >
              清空
            </Button>
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

          {view === 'action' && (
            <div className="flex flex-wrap items-center gap-2 border-t border-line/60 px-2 py-1">
              <Select
                value={outcome}
                title="动作结果"
                onChange={(e) => setOutcome(e.target.value)}
              >
                <option value="">全部结果</option>
                <option value="ok">成功</option>
                <option value="skip">跳过</option>
                <option value="fail">失败</option>
                <option value="partial">部分成功</option>
              </Select>
              <Select
                value={trigger}
                title="触发方"
                onChange={(e) => setTrigger(e.target.value)}
              >
                <option value="">全部触发</option>
                <option value="user">手动</option>
                <option value="auto">自动</option>
                <option value="schedule">定时</option>
                <option value="startup">启动</option>
              </Select>
              <Input
                placeholder="主体 / 账号"
                className="w-28"
                value={subject}
                onChange={(e) => setSubject(e.target.value)}
              />
              <label className="flex items-center gap-1 text-xs text-muted">
                起
                <Input
                  type="datetime-local"
                  className="w-[11.5rem] text-xs"
                  value={from}
                  onChange={(e) => setFrom(e.target.value)}
                />
              </label>
              <label className="flex items-center gap-1 text-xs text-muted">
                止
                <Input
                  type="datetime-local"
                  className="w-[11.5rem] text-xs"
                  value={to}
                  onChange={(e) => setTo(e.target.value)}
                />
              </label>
            </div>
          )}

          <div className="flex flex-wrap items-center gap-3 border-t border-line/60 px-2 py-1 text-xs">
            <span className="text-muted">
              当前结果 {view === 'stat' ? `${groups.data?.length ?? 0} 类` : `${stats.total} 行`}
            </span>
            <button
              className="cursor-pointer text-warn hover:underline"
              onClick={() => {
                setView('all');
                setMinLevel('warn');
                setOutcome('');
                setTrigger('');
                setSubject('');
                setFrom('');
                setTo('');
              }}
            >
              异常 {stats.abnormal}
            </button>
            <button
              className="cursor-pointer text-bad hover:underline"
              onClick={() => {
                setView('action');
                setOutcome('fail');
              }}
            >
              失败动作 {stats.failed}
            </button>
            <button
              className="cursor-pointer text-muted hover:text-fg"
              onClick={() => {
                setView('action');
                setOutcome('skip');
              }}
            >
              跳过 {stats.skipped}
            </button>
            {live.data?.length ? <span className="text-muted">实时 {live.data.length}</span> : null}
          </div>
        </div>

        <div className="p-2">
          {view === 'stat' ? (
            <StatTable
              stats={groups.data ?? []}
              loading={groups.isLoading}
              error={groups.isError ? errText(groups.error) : null}
              onRetry={() => groups.refetch()}
              onPick={(stat) => {
                // 点一类 → 回到明细并按这一类的模块/动作筛出来
                setView('all');
                setModule(stat.module === 'action' ? '' : stat.module);
                setKeyword(stat.action ?? '');
              }}
            />
          ) : (
            <>
              {query.isLoading && <SkeletonRows rows={8} cols={1} />}
              {query.isError && (
                <StateBlock
                  kind="error"
                  title="读日志失败"
                  detail={errText(query.error)}
                  onRetry={() => query.refetch()}
                />
              )}
              {!query.isLoading && rows.length === 0 && (
                <p className="py-6 text-center text-sm text-muted">
                  {view === 'action'
                    ? '还没有执行动作。每次点击、每次定时轮询都会在这里留一条。'
                    : '没有符合条件的日志。'}
                </p>
              )}
              {rows.map((line, i) => (
                <LogRow key={`${line.ts}-${line.module}-${line.action ?? ''}-${i}`} line={line} />
              ))}
              <div ref={tailRef} />
            </>
          )}
        </div>
      </div>
      <div className="flex items-center justify-between text-xs text-muted">
        <span>
          相同内容 60 秒内合并为一条，次数见 ×N
          {view === 'action' && ' · 只显示真实动作与逐项记录，旧版只读命令已隐藏'}
        </span>
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

/** 起始时间：datetime-local 是本地时间，转 RFC3339 由 `toISOString` 完成。 */
function startIso(value: string): string | null {
  return value ? new Date(value).toISOString() : null;
}

/** 结束时间含整分钟：控件只到分钟时，选 09:30 必须包含 09:30:59.999 的日志。 */
function endIso(value: string): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (value.length <= 16) date.setSeconds(59, 999);
  return date.toISOString();
}

/** RFC3339 转 YYYY-MM-DD HH:mm:ss.SSS：跨年也要能分清，毫秒保留先后顺序。 */
function shortTs(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const p = (n: number) => String(n).padStart(2, '0');
  return (
    `${d.getFullYear()}-` +
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

function effectiveOutcome(line: LogLine): string | null {
  if (line.outcome) return line.outcome;
  if (line.ok === true) return 'ok';
  if (line.ok === false) return 'fail';
  return null;
}

/** 实时行走与后端查询同一套过滤语义。 */
function matchFilter(line: LogLine, filter: LogFilter): boolean {
  if (filter.level && line.level !== filter.level) return false;
  if (filter.min_level && LEVEL_RANK[line.level] < LEVEL_RANK[filter.min_level]) return false;
  if (filter.module && !line.module.includes(filter.module)) return false;
  if (filter.action_only && line.module !== 'action') return false;
  if (filter.outcome && effectiveOutcome(line) !== filter.outcome) return false;
  if (filter.trigger && line.trigger !== filter.trigger) return false;
  if (filter.subject && !(line.subject ?? '').includes(filter.subject)) return false;
  if (filter.from || filter.to) {
    const ts = new Date(line.ts).getTime();
    if (Number.isNaN(ts)) return false;
    if (filter.from && ts < new Date(filter.from).getTime()) return false;
    if (filter.to && ts > new Date(filter.to).getTime()) return false;
  }
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

/**
 * 一行日志：抬头（时间/级别/主体/动作/结果/耗时）+ 正文 + 结构化字段。
 * 结构化字段单独一行而不是塞进正文：排障时唯一有用的部分不能被长句截断。
 */
function LogRow({ line }: { line: LogLine }) {
  const outcomeKey = effectiveOutcome(line);
  const outcome = outcomeKey ? OUTCOME[outcomeKey] : null;
  const failed = line.level === 'error' || line.level === 'warn';
  // 历史行里仍有整段 JSON：默认收起到三行，需要时再展开，避免一条日志淹一屏
  const long = line.message.length > 360;
  const [expanded, setExpanded] = useState(false);
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
        {/* 合并行：同一句话重复 N 次只留这一条，次数必须显眼，否则会被当成「只发生了一次」 */}
        {line.repeat != null && line.repeat > 1 && (
          <span
            className="rounded bg-accent/15 px-1 text-[10px] font-semibold tabular-nums text-accent"
            title={
              line.last_ts
                ? `相同内容合并 ${line.repeat} 次，最近一次 ${shortTs(line.last_ts)}`
                : `相同内容合并 ${line.repeat} 次`
            }
          >
            ×{line.repeat}
          </span>
        )}
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
          long && !expanded && 'line-clamp-3',
        )}
      >
        {line.message}
      </div>
      {long && (
        <button
          className="mt-0.5 cursor-pointer pl-1 text-[11px] text-accent hover:underline"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? '收起' : `展开全文（${line.message.length} 字）`}
        </button>
      )}
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

/**
 * 聚合表：一类日志一行。
 * 次数按合并前的真实发生数算（`×N` 已展开），否则「同一条错 500 次」只会显示 1。
 */
function StatTable({
  stats,
  loading,
  error,
  onRetry,
  onPick,
}: {
  stats: LogStat[];
  loading: boolean;
  error: string | null;
  onRetry: () => void;
  onPick: (stat: LogStat) => void;
}) {
  if (loading) return <SkeletonRows rows={6} cols={4} />;
  if (error) return <StateBlock kind="error" title="聚合失败" detail={error} onRetry={onRetry} />;
  if (stats.length === 0) {
    return <p className="py-6 text-center text-sm text-muted">没有可聚合的日志。</p>;
  }
  return (
    <table className="w-full border-collapse text-xs">
      <thead>
        <tr className="text-left text-muted">
          <th className="w-16 py-1 font-medium">次数</th>
          <th className="w-14 py-1 font-medium">级别</th>
          <th className="w-40 py-1 font-medium">模块 / 动作</th>
          <th className="w-16 py-1 font-medium">结果</th>
          <th className="w-36 py-1 font-medium">最近一次</th>
          <th className="py-1 font-medium">代表内容</th>
        </tr>
      </thead>
      <tbody>
        {stats.map((stat, index) => {
          const outcome = stat.outcome ? OUTCOME[stat.outcome] : null;
          return (
            <tr
              key={`${stat.module}-${stat.action ?? ''}-${stat.outcome ?? ''}-${stat.level}-${index}`}
              className="cursor-pointer border-t border-line hover:bg-hover"
              onClick={() => onPick(stat)}
              title="点击回到明细，并按这一类的模块与动作筛选"
            >
              <td className="py-1 font-semibold tabular-nums">{stat.occurrences}</td>
              <td className="py-1">
                <span
                  className={cn(
                    'rounded px-1 text-[10px] font-semibold uppercase',
                    stat.level === 'error'
                      ? 'bg-bad/15 text-bad'
                      : stat.level === 'warn'
                        ? 'bg-warn/15 text-warn'
                        : 'bg-hover text-muted',
                  )}
                >
                  {stat.level}
                </span>
              </td>
              <td className="truncate py-1" title={stat.action ?? stat.module}>
                {stat.action ?? stat.module}
                {stat.action && <span className="ml-1 text-muted">{stat.module}</span>}
              </td>
              <td className={cn('py-1', outcome?.className ?? 'text-muted')}>
                {outcome?.text ?? '—'}
              </td>
              <td className="py-1 tabular-nums text-muted">{shortTs(stat.last_ts)}</td>
              <td className="max-w-[420px] truncate py-1 text-fg2" title={stat.sample}>
                {stat.sample}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
