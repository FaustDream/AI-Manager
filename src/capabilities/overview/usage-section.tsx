// 用量：本地只读采集出来的 Token 与平台扣费。
//
// 与上面的积分收支不是同一份账：这份来自客户端落盘文件（每 30 分钟增量扫描），
// 能归因到具体软件与模型；积分收支来自账号余额快照，回答的是「进出多少」。
// 两块并排放，才能对出「跑了多少 Token」与「扣了多少积分」的关系。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { Button, cn } from '@/components/ui/button';
import { StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { fmtAmount, fmtTime, fmtTokens, fmtTokensExact } from '@/lib/format';
import { appLabel, ModelDonut, TrendLines } from '@/components/usage-charts';
import type { UsageOverview } from '@/lib/types';
import { Metric, SectionTitle } from './metric';

export function UsageSection() {
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
    <section className="rounded-lg border border-line bg-card p-3">
      <SectionTitle
        right={
          <span className="flex flex-wrap items-center gap-1">
            {data?.scanned_at && <span className="mr-1">上次采集 {fmtTime(data.scanned_at)}</span>}
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
          </span>
        }
      >
        用量（本地文件采集）
      </SectionTitle>

      {overview.isLoading && (
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          {Array.from({ length: 4 }).map((_, index) => (
            <div key={index} className="h-[92px] animate-pulse rounded-lg bg-hover" />
          ))}
        </div>
      )}

      {overview.isError && (
        <StateBlock
          kind="error"
          title="读用量失败"
          detail={errText(overview.error)}
          onRetry={() => overview.refetch()}
        />
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
        <div className="flex flex-col gap-3">
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
            <div className="rounded-lg border border-line/70 p-2">
              <div className="mb-1.5 flex items-baseline gap-2">
                <h4 className="text-xs font-semibold tracking-wide text-fg2">Token 消耗趋势</h4>
                <span className="flex-1" />
                <span className="text-xs text-muted">每天一个点 · 按软件分线</span>
              </div>
              <TrendLines days={data.days} />
            </div>

            <div className="rounded-lg border border-line/70 p-2">
              <div className="mb-1.5 flex items-baseline gap-2">
                <h4 className="text-xs font-semibold tracking-wide text-fg2">模型占比</h4>
                <span className="flex-1" />
                <span className="text-xs text-muted">按 Token</span>
              </div>
              <ModelDonut models={data.by_model} />
            </div>
          </div>

          <div>
            <div className="mb-1.5 flex items-baseline gap-2">
              <h4 className="text-xs font-semibold tracking-wide text-fg2">各软件用量</h4>
              <span className="flex-1" />
              <span className="text-xs text-muted">最近 {days} 天 · 平台扣费口径</span>
            </div>
            <table className="w-full border-collapse text-xs">
              <thead>
                <tr className="text-left text-muted">
                  <th className="py-1 font-medium">软件</th>
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
                      <td className="whitespace-nowrap py-1">{appLabel(app.platform)}</td>
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
          </div>

          <div>
            <div className="mb-1.5 flex items-baseline gap-2">
              <h4 className="text-xs font-semibold tracking-wide text-fg2">每日明细</h4>
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
                        <td className="whitespace-nowrap py-1 tabular-nums">{day.date}</td>
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
          </div>
        </div>
      )}
    </section>
  );
}
