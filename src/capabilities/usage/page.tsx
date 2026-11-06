// 用量：本地只读采集出来的 Token / 积分消耗。
//
// 数据不是实时的——采集器每 30 分钟扫一遍客户端落盘文件，页面只读本地库。
// 所以「数据更新于」比「刚刚」更重要：它决定了你看到的是几分钟前还是几小时前。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { Button, cn } from '@/components/ui/button';
import { StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { fmtAmount, fmtTime, fmtTokens, fmtTokensExact } from '@/lib/format';
import { appLabel, ModelDonut, StackedArea } from '@/components/usage-charts';
import type { UsageOverview } from '@/lib/types';

function ago(ms: number | null): string {
  if (ms === null) return '从未采集';
  const diff = Date.now() - ms;
  if (diff < 60_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  return `${Math.floor(diff / 86_400_000)} 天前`;
}

function Metric({
  label,
  value,
  hint,
  exact,
}: {
  label: string;
  value: string;
  hint?: string;
  /** 换算成亿/万后的小数有取舍，悬浮给原数。 */
  exact?: string;
}) {
  return (
    <div className="rounded-lg border border-line bg-card p-3">
      <div className="text-xs text-muted">{label}</div>
      <div className="mt-1 text-2xl font-semibold tabular-nums" title={exact}>
        {value}
      </div>
      {hint && <div className="mt-0.5 truncate text-xs text-muted">{hint}</div>}
    </div>
  );
}

export default function UsagePage() {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const [days, setDays] = useState(14);

  const overview = useQuery<UsageOverview>({
    queryKey: ['usage', days],
    queryFn: () => queryCmd('usage_overview', { days }),
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5 * 60_000),
  });

  const scan = () =>
    run('scan', async () => {
      const report = await runCmd('usage_scan');
      if (report === undefined) return;
      const failed = report.errors.length;
      toast(
        `扫描完成：新增 ${report.inserted} 条 · 扫描 ${report.scanned} 条` +
          (failed ? `\n${failed} 个源失败：${report.errors[0]}` : ''),
      );
      await qc.invalidateQueries({ queryKey: ['usage'] });
    });

  const data = overview.data;
  const empty = data && data.total.calls === 0;
  // days 已把区间补满且按日期升序，所以「今天的前一天」就是倒数第二个
  const yesterday = data && data.days.length >= 2 ? data.days[data.days.length - 2] : null;
  const dayDelta =
    data && yesterday && yesterday.tokens > 0
      ? Math.round(((data.today.tokens - yesterday.tokens) / yesterday.tokens) * 100)
      : null;
  const perCall = data && data.total.calls > 0 ? Math.round(data.total.tokens / data.total.calls) : 0;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <h1 className="text-lg font-semibold">用量</h1>
        {data && (
          <span className="text-xs text-muted">
            数据更新于 {ago(data.scanned_at)} · 本地只读采集
          </span>
        )}
        <span className="flex-1" />
        <span className="flex items-center gap-0.5 rounded border border-line p-0.5">
          {[7, 14, 30].map((value) => (
            <button
              key={value}
              className={cn(
                'cursor-pointer rounded px-1.5 py-0.5 text-xs',
                days === value ? 'bg-hover text-fg' : 'text-muted hover:text-fg',
              )}
              onClick={() => setDays(value)}
            >
              {value} 天
            </button>
          ))}
        </span>
        <Button size="sm" onClick={scan} loading={busy === 'scan'}>
          {busy === 'scan' ? '扫描中…' : '扫描本地文件'}
        </Button>
      </div>

      {overview.isLoading && (
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          {Array.from({ length: 4 }).map((_, index) => (
            <div key={index} className="h-[92px] animate-pulse rounded-lg bg-hover" />
          ))}
        </div>
      )}

      {overview.isError && (
        <div className="rounded-lg border border-line bg-card">
          <StateBlock
            kind="error"
            title="读用量失败"
            detail={errText(overview.error)}
            onRetry={() => overview.refetch()}
          />
        </div>
      )}

      {empty && (
        <div className="rounded-lg border border-dashed border-line">
          <StateBlock
            kind="empty"
            title="还没有用量数据"
            detail="采集器只读客户端本地文件，不改任何配置。点「扫描本地文件」立刻跑一次；之后每 30 分钟自动增量扫描。"
          />
          <div className="pb-6 text-center">
            <Button size="sm" variant="primary" onClick={scan} loading={busy === 'scan'}>
              扫描本地文件
            </Button>
          </div>
        </div>
      )}

      {data && !empty && (
        <>
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
            <Metric
              label="今日 Token"
              value={fmtTokens(data.today.tokens)}
              exact={fmtTokensExact(data.today.tokens)}
              hint={
                `${data.today.calls} 次调用` +
                (dayDelta === null ? '' : ` · 较昨日 ${dayDelta > 0 ? '+' : ''}${dayDelta}%`)
              }
            />
            <Metric
              label="今日积分"
              value={fmtAmount(data.today.credit)}
              hint="平台实际扣费口径"
            />
            <Metric
              label="区间 Token"
              value={fmtTokens(data.total.tokens)}
              exact={fmtTokensExact(data.total.tokens)}
              hint={`最近 ${days} 天 · 日均 ${fmtTokens(Math.round(data.total.tokens / days))}`}
            />
            <Metric
              label="区间积分"
              value={fmtAmount(data.total.credit)}
              hint={`${data.total.calls} 次调用 · 平均每次 ${fmtTokens(perCall)} Token`}
            />
          </div>

          <div className="grid gap-3 lg:grid-cols-[minmax(0,1fr)_320px]">
            <section className="rounded-lg border border-line bg-card p-3">
              <div className="mb-1.5 flex items-baseline gap-2">
                <h3 className="text-xs font-semibold tracking-wide text-fg2">Token 消耗趋势</h3>
                <span className="flex-1" />
                <span className="text-xs text-muted">每根柱子 = 一天 · 按平台堆叠</span>
              </div>
              <StackedArea days={data.days} />
            </section>

            <section className="rounded-lg border border-line bg-card p-3">
              <div className="mb-1.5 flex items-baseline gap-2">
                <h3 className="text-xs font-semibold tracking-wide text-fg2">模型占比</h3>
                <span className="flex-1" />
                <span className="text-xs text-muted">按 Token</span>
              </div>
              <ModelDonut models={data.by_model} />
            </section>
          </div>

          <section className="rounded-lg border border-line bg-card p-3">
            <div className="mb-1.5 flex items-baseline gap-2">
              <h3 className="text-xs font-semibold tracking-wide text-fg2">各应用消耗</h3>
              <span className="flex-1" />
              <span className="text-xs text-muted">最近 {days} 天</span>
            </div>
            <table className="w-full border-collapse text-xs">
              <thead>
                <tr className="text-left text-muted">
                  <th className="py-1 font-medium">平台</th>
                  <th className="py-1 text-right font-medium">Token</th>
                  <th className="py-1 text-right font-medium">积分</th>
                  <th className="py-1 text-right font-medium">调用</th>
                  <th className="w-32 py-1 pl-3 font-medium">占比</th>
                </tr>
              </thead>
              <tbody>
                {data.by_app.map((app) => {
                  const share = data.total.tokens === 0 ? 0 : (app.tokens / data.total.tokens) * 100;
                  return (
                    <tr key={app.platform} className="border-t border-line/60">
                      <td className="py-1">{appLabel(app.platform)}</td>
                      <td
                        className="py-1 text-right tabular-nums"
                        title={fmtTokensExact(app.tokens)}
                      >
                        {fmtTokens(app.tokens)}
                      </td>
                      <td className="py-1 text-right tabular-nums">{fmtAmount(app.credit)}</td>
                      <td className="py-1 text-right tabular-nums">{app.calls}</td>
                      <td className="py-1 pl-3">
                        <div className="flex items-center gap-1.5">
                          <div className="h-1.5 flex-1 rounded-full bg-hover">
                            <div
                              className="h-1.5 rounded-full bg-accent"
                              style={{ width: `${Math.max(2, share)}%` }}
                            />
                          </div>
                          <span className="w-10 text-right tabular-nums text-muted">
                            {share.toFixed(0)}%
                          </span>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </section>
        </>
      )}

      {data && !empty && (
        <section className="rounded-lg border border-line bg-card p-3">
          <div className="mb-1.5 flex items-baseline gap-2">
            <h3 className="text-xs font-semibold tracking-wide text-fg2">每日明细</h3>
            <span className="flex-1" />
            <span className="text-xs text-muted">最近 {days} 天 · 最新在上</span>
          </div>
          {/* 区间内每天都有行（没记录的是 0），这样才能一眼看出「哪几天真的没用」 */}
          <div className="max-h-72 overflow-auto">
            <table className="w-full border-collapse text-xs">
              <thead>
                <tr className="sticky top-0 bg-card text-left text-muted">
                  <th className="py-1 font-medium">日期</th>
                  <th className="py-1 text-right font-medium">Token</th>
                  <th className="py-1 text-right font-medium">积分</th>
                  <th className="py-1 text-right font-medium">调用</th>
                  <th className="w-40 py-1 pl-3 font-medium">占区间</th>
                </tr>
              </thead>
              <tbody>
                {[...data.days].reverse().map((day) => {
                  const share = data.total.tokens === 0 ? 0 : (day.tokens / data.total.tokens) * 100;
                  return (
                    <tr key={day.date} className="border-t border-line/60">
                      <td className="py-1 tabular-nums">{day.date}</td>
                      <td
                        className="py-1 text-right tabular-nums"
                        title={fmtTokensExact(day.tokens)}
                      >
                        {fmtTokens(day.tokens)}
                      </td>
                      <td className="py-1 text-right tabular-nums">
                        {day.credit > 0 ? fmtAmount(day.credit) : '—'}
                      </td>
                      <td className="py-1 text-right tabular-nums">{day.calls}</td>
                      <td className="py-1 pl-3">
                        <div className="flex items-center gap-1.5">
                          <div className="h-1.5 flex-1 rounded-full bg-hover">
                            <div
                              className="h-1.5 rounded-full bg-accent"
                              style={{ width: share > 0 ? `${Math.max(2, share)}%` : '0%' }}
                            />
                          </div>
                          <span className="w-10 text-right tabular-nums text-muted">
                            {share.toFixed(0)}%
                          </span>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </section>
      )}

      {data && data.scanned_at && (
        <div className="text-right text-xs text-muted">
          上次采集 {fmtTime(data.scanned_at)}
        </div>
      )}
    </div>
  );
}
