// 执行记录：谁在什么时候干了什么、成没成、花了多久。
//
// 数据来自 app.jsonl 里的动作行（`module == "action"`），和「日志」页共用一份文件：
// 日志页看全量流水，这里只看动作——排查「为什么失败」时先看这一页，
// 需要上下文再跳到日志页按时间对齐。
import { useQuery } from '@tanstack/react-query';
import { queryCmd, runCmd } from '@/lib/cmd';
import { useEffect, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Input, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { errText } from '@/lib/ipc';
import { cn } from '@/components/ui/button';
import type { LogFilter, LogLine } from '@/lib/types';

/**
 * 旧版本把只读命令也记成了动作（界面就绪、读列表…），这些行还留在日志里。
 * 展示时挡掉——日志是历史资产，不该为了好看去改文件，但也不该继续占视线。
 */
const LEGACY_NOISE = new Set([
  'ui_ready',
  'account_list',
  'account_current',
  'account_uid',
  'account_codebuddy_targets',
  'hub_snapshot',
  'session_page',
  'session_preview',
  'session_keys',
  'session_facets',
  'session_trash',
  'session_backups',
  'session_retention',
  'session_running_clients',
  'checkin_detail',
  'daily_state',
  'log_query',
  'settings_get',
  'autostart_get',
  'schedule_list',
  'maintenance_space',
  'maintenance_trash',
]);

const TRIGGER_LABEL: Record<string, string> = {
  user: '手动',
  schedule: '定时',
  startup: '启动',
};

/** 结果：跳过 ≠ 失败，部分成功要单独一档——混在一起就判断不了要不要人工介入。 */
const OUTCOME: Record<string, { text: string; className: string }> = {
  ok: { text: '成功', className: 'text-ok' },
  skip: { text: '跳过', className: 'text-muted' },
  fail: { text: '失败', className: 'text-bad' },
  partial: { text: '部分成功', className: 'text-warn' },
};

function outcomeOf(line: LogLine): { text: string; className: string } {
  const key = line.outcome ?? (line.ok === false ? 'fail' : line.ok === true ? 'ok' : '');
  return OUTCOME[key] ?? { text: '—', className: 'text-muted' };
}

export default function ExecutionsPage() {
  const [onlyFailed, setOnlyFailed] = useState(false);
  const [keyword, setKeyword] = useState('');
  const [limit, setLimit] = useState(300);

  const filter: LogFilter = {
    level: null,
    // 只看动作行：普通业务日志在「日志」页
    module: 'action',
    keyword: keyword || null,
    limit,
  };
  const query = useQuery<LogLine[]>({
    queryKey: ['executions', JSON.stringify(filter)],
    queryFn: () => queryCmd('log_query', { filter }),
  });
  // 实时行由 providers 的事件桥写进 ['log-live']，这里只挑动作行
  const live = useQuery<LogLine[]>({
    queryKey: ['log-live'],
    queryFn: () => [],
    staleTime: Infinity,
  });

  const matches = (line: LogLine) => {
    if (!line.action || LEGACY_NOISE.has(line.action)) return false;
    if (onlyFailed && line.ok !== false) return false;
    if (keyword && !line.message.includes(keyword) && !line.action.includes(keyword)) return false;
    return true;
  };

  const merged = [...(query.data ?? []), ...(live.data ?? [])].filter(matches);
  // 同一行可能既在查询结果里又在实时缓存里：按「时间+动作」去重。
  //
  // 保持**时间正序**：一个批次是「开始 → 逐账号 → 结束」，倒序会把整批读反。
  // 最新一条靠自动贴底看到，和「日志」页同一套习惯。
  const seen = new Set<string>();
  const rows = merged.filter((line) => {
    const key = `${line.ts}|${line.action}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  const failed = rows.filter((r) => r.ok === false).length;

  // 贴底跟随：新记录永远在最后一行，不跟着滚就得手动翻
  const tailRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    tailRef.current?.scrollIntoView({ block: 'end' });
  }, [rows.length]);

  return (
    <div className="flex h-[calc(100vh-7.5rem)] flex-col gap-2">
      <div className="min-h-0 flex-1 overflow-auto rounded-lg border border-line bg-card">
        <div className="sticky top-0 z-10 flex flex-wrap items-center gap-2 border-b border-line bg-bg px-2 py-1.5">
          <Input
            placeholder="关键词 / 动作"
            className="w-40"
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
          />
          <Select value={String(limit)} onChange={(e) => setLimit(Number(e.target.value) || 300)}>
            <option value="100">最近 100 条</option>
            <option value="300">最近 300 条</option>
            <option value="1000">最近 1000 条</option>
          </Select>
          <label className="flex cursor-pointer items-center gap-1 text-xs text-fg2">
            <input
              type="checkbox"
              checked={onlyFailed}
              onChange={(e) => setOnlyFailed(e.target.checked)}
            />
            只看失败
          </label>
          <span className="flex-1" />
          <span className="text-xs text-muted">
            共 {rows.length} 条{failed > 0 ? ` · 失败 ${failed}` : ''}
          </span>
          <Button
            size="sm"
            onClick={async () => {
              const path = await runCmd('log_export', { path: null, filter });
              if (path !== undefined) toast(path);
            }}
          >
            导出
          </Button>
        </div>

        {query.isLoading && <SkeletonRows rows={8} cols={3} />}
        {query.isError && (
          <StateBlock
            kind="error"
            title="读执行记录失败"
            detail={errText(query.error)}
            onRetry={() => query.refetch()}
          />
        )}
        {!query.isLoading && rows.length === 0 && (
          <p className="py-8 text-center text-sm text-muted">
            还没有执行记录。每次点击、每次定时轮询都会在这里留一条。
          </p>
        )}

        {rows.length > 0 && (
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="text-left text-xs text-muted">
                <th className="w-28 px-2 py-1 font-medium">时间</th>
                <th className="w-28 px-2 py-1 font-medium">主体</th>
                <th className="w-20 px-2 py-1 font-medium">动作</th>
                <th className="w-20 px-2 py-1 font-medium">结果</th>
                <th className="w-16 px-2 py-1 text-right font-medium">耗时</th>
                <th className="px-2 py-1 font-medium">详情</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((line, index) => (
                <tr
                  key={`${line.ts}-${line.action}-${index}`}
                  className="border-t border-line/60 align-top"
                >
                  <td className="px-2 py-1 tabular-nums text-muted">
                    {shortTs(line.ts)}
                    {line.trigger && (
                      <span className="ml-1 text-[10px]">{TRIGGER_LABEL[line.trigger] ?? line.trigger}</span>
                    )}
                  </td>
                  <td className="max-w-28 truncate px-2 py-1" title={line.subject ?? undefined}>
                    {line.subject ?? '—'}
                  </td>
                  <td className="px-2 py-1 text-xs text-fg2" title={line.action ?? undefined}>
                    {line.action}
                  </td>
                  <td className="px-2 py-1">
                    <span className={cn('text-xs font-medium', outcomeOf(line).className)}>
                      {outcomeOf(line).text}
                    </span>
                  </td>
                  <td className="px-2 py-1 text-right tabular-nums text-muted">{fmtMs(line.ms)}</td>
                  <td
                    className="line-clamp-4 px-2 py-1 break-words"
                    title={line.message}
                  >
                    {line.message}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <div ref={tailRef} />
      </div>
      <p className="text-xs text-muted">
        只记改变状态的动作（切换、删除、签到、改配置…）；读列表、开窗口这类不进这里。
        详情可悬停看全文，完整上下文去「日志」页按时间对齐。
      </p>
    </div>
  );
}

/** 毫秒转人读：低于 1 秒给 ms，超过给秒。 */
function fmtMs(ms: number | null): string {
  if (ms === null || ms === undefined) return '—';
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

/** RFC3339 转 MM-DD HH:mm:ss。 */
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
    p(d.getSeconds())
  );
}
