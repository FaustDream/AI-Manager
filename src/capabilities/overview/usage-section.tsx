// 用量：本地只读采集出来的 Token 与平台扣费。
//
// 按 dashboard-design 技能重构：KPI 瓦片只留数字与 delta 徽章（口径与解释进 tooltip）；
// 各软件用量与模型占比是排名列表 + 行底 3px 份额条（环形图只配「一家独大」的份额，
// 七个切片的占比用排名列表更诚实）；每日明细是按平台堆叠柱（零日自然缺失，不装没数据）。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { fmtAmount, fmtTime, fmtTokens, fmtTokensExact } from '@/lib/format';
import { appLabel, DailyTokensBars, TrendLines } from '@/components/usage-charts';
import type { UsageModel, UsageOverview, UsageSlice } from '@/lib/types';
import { Delta, Metric, SectionTitle, ShareBar, shareText } from './metric';

export function UsageSection() {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  // 默认今天：与上面的积分收支同一个默认口径
  const [days, setDays] = useState(1);

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
  const perCall = data && data.total.calls > 0 ? Math.round(data.total.tokens / data.total.calls) : 0;

  return (
    <section className="rounded-lg border border-line bg-card p-3">
      <SectionTitle
        right={
          <span className="flex flex-wrap items-center gap-1">
            {data?.scanned_at && <span className="mr-1">上次采集 {fmtTime(data.scanned_at)}</span>}
            <span className="flex items-center gap-0.5 rounded border border-line p-0.5">
              {[1, 7, 14, 30].map((value) => (
                <button
                  key={value}
                  className={
                    days === value
                      ? 'rounded bg-hover px-1.5 py-0.5 text-xs text-fg'
                      : 'cursor-pointer rounded px-1.5 py-0.5 text-xs text-muted hover:text-fg'
                  }
                  onClick={() => setDays(value)}
                >
                  {value === 1 ? '今天' : `${value} 天`}
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
            title={days === 1 ? '今天还没有用量数据' : '还没有用量数据'}
            detail={
              days === 1
                ? '默认只看今天；切到 7 / 14 / 30 天可看历史区间。采集器只读客户端本地文件，不改任何配置，之后每 30 分钟自动增量扫描。'
                : '采集器只读客户端本地文件，不改任何配置。点「扫描本地文件」立刻跑一次；之后每 30 分钟自动增量扫描。'
            }
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
              hint={`${data.today.calls} 次调用`}
              delta={
                <Delta
                  cur={data.today.tokens}
                  prev={yesterday && yesterday.tokens > 0 ? yesterday.tokens : 0}
                />
              }
            />
            <Metric
              label="今日积分"
              value={fmtAmount(data.today.credit)}
              hint={
                data.today.calls > 0
                  ? `平台实际扣费口径 · ${data.today.calls} 次调用`
                  : '平台实际扣费口径'
              }
            />
            <Metric
              label="区间 Token"
              value={fmtTokens(data.total.tokens)}
              exact={fmtTokensExact(data.total.tokens)}
              hint={`日均 ${fmtTokens(Math.round(data.total.tokens / days))}`}
            />
            <Metric
              label="区间积分"
              value={fmtAmount(data.total.credit)}
              hint={
                data.total.calls > 0
                  ? `平台实际扣费口径 · ${data.total.calls} 次调用 · 单次 ${fmtTokens(perCall)} Token`
                  : '平台实际扣费口径'
              }
            />
          </div>

          <div className="rounded-lg border border-line/70 p-3">
            <div className="mb-1.5 flex items-baseline gap-2">
              <h4 className="text-xs font-semibold tracking-wide text-fg2">Token 消耗趋势</h4>
              <span className="flex-1" />
            </div>
            <TrendLines days={data.days} />
          </div>

          <div className="grid gap-3 lg:grid-cols-2">
            <div className="rounded-lg border border-line/70 p-3">
              <div className="mb-1 flex items-baseline justify-between gap-2 px-1 text-xs text-muted">
                <span>软件</span>
                <span>Token · 占比</span>
              </div>
              <RankedAppUsage byApp={data.by_app} totalTokens={data.total.tokens} />
            </div>

            <div className="rounded-lg border border-line/70 p-3">
              <div className="mb-1 flex items-baseline justify-between gap-2 px-1 text-xs text-muted">
                <span>模型</span>
                <span>Token · 占比</span>
              </div>
              <RankedModelUsage byModel={data.by_model} totalTokens={data.total.tokens} />
            </div>
          </div>

          <div className="rounded-lg border border-line/70 p-3">
            <div className="mb-1.5 flex items-baseline gap-2">
              <h4 className="text-xs font-semibold tracking-wide text-fg2">每日明细</h4>
              <span className="flex-1" />
            </div>
            <DailyTokensBars days={data.days} />
          </div>
        </div>
      )}
    </section>
  );
}

/** 排名列表：一行一个软件，数值右对齐、行底 3px 份额条，hover 提亮。 */
function RankedAppUsage({ byApp, totalTokens }: { byApp: UsageSlice[]; totalTokens: number }) {
  const rows = [...byApp].sort((a, b) => b.tokens - a.tokens);
  const max = Math.max(...rows.map((row) => row.tokens), 1);
  return (
    <div>
      {rows.map((row) => (
        <button
          key={row.platform}
          title={`${appLabel(row.platform)} · ${fmtTokensExact(row.tokens)} Token · ${row.calls} 次调用 · 积分 ${
            row.credit > 0 ? fmtAmount(row.credit) : '—'
          }`}
          className="group relative flex h-11 w-full items-center gap-3 rounded-[10px] px-3 pb-1.5 text-left text-sm text-fg hover:bg-hover/60"
        >
          <ShareBar pct={(row.tokens / max) * 100} />
          <span className="relative min-w-0 flex-1 truncate">{appLabel(row.platform)}</span>
          <span className="relative tabular-nums" title={fmtTokensExact(row.tokens)}>
            {fmtTokens(row.tokens)}
          </span>
          <span className="relative w-10 text-right text-xs tabular-nums text-muted">
            {shareText(totalTokens === 0 ? 0 : (row.tokens / totalTokens) * 100)}
          </span>
        </button>
      ))}
    </div>
  );
}

/** 模型占比：同样是排名列表（环形图只配「一家独大」的份额）。 */
function RankedModelUsage({ byModel, totalTokens }: { byModel: UsageModel[]; totalTokens: number }) {
  const rows = [...byModel].sort((a, b) => b.tokens - a.tokens);
  const max = Math.max(...rows.map((row) => row.tokens), 1);
  return (
    <div>
      {rows.map((row) => (
        <div
          key={row.model}
          title={`${row.model} · ${fmtTokensExact(row.tokens)} Token · ${row.calls} 次调用`}
          className="group relative flex h-11 w-full items-center gap-3 rounded-[10px] px-3 pb-1.5 text-left text-sm text-fg hover:bg-hover/60"
        >
          <ShareBar pct={(row.tokens / max) * 100} />
          <span className="relative min-w-0 flex-1 truncate" title={row.model}>
            {row.model}
          </span>
          <span className="relative tabular-nums" title={fmtTokensExact(row.tokens)}>
            {fmtTokens(row.tokens)}
          </span>
          <span className="relative w-10 text-right text-xs tabular-nums text-muted">
            {shareText(totalTokens === 0 ? 0 : (row.tokens / totalTokens) * 100)}
          </span>
        </div>
      ))}
    </div>
  );
}
