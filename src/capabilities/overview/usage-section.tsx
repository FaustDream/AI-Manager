// Token 统计：本地只读采集出来的 Token 与平台扣费。
//
// 版式与左栏的积分统计卡**同构**：标题行 + 2×2 数字瓦片 + 两个子块
// （消耗趋势 / 软件·模型排名）。同构是硬要求——两栏卡片要能对齐，结构就得一样。
// 区间由页头的全局过滤统一给出，卡内不再挂控件。
// 采集全是后台行为：启动后自动扫一遍，之后按设置的间隔增量扫描，界面上不给手动入口。
// 逐日明细不在这张卡里：趋势线已经按月-日给出每天的合计，再堆一张整页宽的柱状图
// 是把同一份数据画两遍。
//
// 两块共用 useUsageQuery 的一次请求：queryKey 只由区间决定，react-query 自己去重。
// 按 dashboard-design 技能：瓦片只留数字与 delta 徽章（口径进 tooltip）；
// 占比用排名列表 + 行底 3px 份额条，不用环形图（七个切片读不出差异）。
import { useQuery, type UseQueryResult } from '@tanstack/react-query';
import { StateBlock } from '@/components/ui/form';
import { queryCmd } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { fmtAmount, fmtTime, fmtTokens, fmtTokensExact } from '@/lib/format';
import { appLabel, TrendLines } from '@/components/usage-charts';
import type { UsageModel, UsageOverview, UsageSlice } from '@/lib/types';
import { Metric, SectionTitle, ShareBar, shareText } from './metric';
import type { Range } from './range';

/** 用量查询：页面持有区间，卡内的瓦片与趋势图共用这一次请求。 */
export function useUsageQuery(range: Range): UseQueryResult<UsageOverview> {
  return useQuery<UsageOverview>({
    queryKey: ['usage', range.from, range.to],
    queryFn: () => queryCmd('usage_overview', { from: range.from, to: range.to }),
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5 * 60_000),
  });
}

export function UsageSection({
  query,
  range,
}: {
  query: UseQueryResult<UsageOverview>;
  range: Range;
}) {
  const data = query.data;
  const empty = data !== undefined && data.total.calls === 0;
  // 后端把区间内每一天都补满且按日期升序，天数就是区间跨度
  const days = data?.days.length ?? 0;
  const perCall = data && data.total.calls > 0 ? Math.round(data.total.tokens / data.total.calls) : 0;

  return (
    // 四行 subgrid（标题 / 数字 / 子块一 / 子块二）：与左栏共用父栅格的行高，
    // 所以两栏的子块边界必然对齐。多一个直接子节点就会顶错行，条件块必须收在容器里。
    <section className="grid min-w-0 gap-3 p-3 xl:row-span-4 xl:grid-rows-subgrid">
      <div>
        {/* 与左栏同理：区间控件归页头「全局过滤」，这里保留行高让两卡数字行对齐 */}
        <SectionTitle className="mb-0 min-h-7 items-center">Token 统计</SectionTitle>
      </div>

      {query.isLoading && (
        <div className="grid gap-3">
          <div className="grid grid-cols-2 gap-3">
            {Array.from({ length: 4 }).map((_, index) => (
              <div key={index} className="h-[92px] skeleton-block rounded-lg bg-hover" />
            ))}
          </div>
          <div className="h-[300px] skeleton-block rounded-lg bg-hover" />
        </div>
      )}

      {query.isError && (
        <StateBlock
          kind="error"
          title="读用量失败"
          detail={errText(query.error)}
          onRetry={() => query.refetch()}
        />
      )}

      {empty && (
        <div>
          <StateBlock
            kind="empty"
            title={range.key === '1' ? '今天还没有用量数据' : '这个区间没有用量数据'}
            detail="采集全自动：启动后立刻扫一遍，之后按设置的间隔（默认 30 分钟）增量扫描。只读客户端本地文件，不改任何配置。"
          />
        </div>
      )}

      {data && !empty && (
        <>
          {/* 四个瓦片全部跟区间走：换区间时四个数一起变，不会出现「两个数变了、两个没变」 */}
          <div className="grid grid-cols-2 gap-3">
            <Metric
              label={`Token · ${range.label}`}
              value={fmtTokens(data.total.tokens)}
              exact={fmtTokensExact(data.total.tokens)}
              hint={`区间内 ${data.total.calls} 次调用`}
            />
            <Metric
              label={`积分 · ${range.label}`}
              value={fmtAmount(data.total.credit)}
              hint={
                data.total.calls > 0
                  ? `平台实际扣费口径 · ${data.total.calls} 次调用`
                  : '平台实际扣费口径'
              }
            />
            <Metric
              label="日均 Token"
              value={fmtTokens(days > 0 ? Math.round(data.total.tokens / days) : 0)}
              exact={fmtTokensExact(days > 0 ? Math.round(data.total.tokens / days) : 0)}
              hint={`区间 ${days} 天的日均（含没有用量的日子）`}
            />
            <Metric
              label="单次平均 Token"
              value={fmtTokens(perCall)}
              exact={fmtTokensExact(perCall)}
              hint={`区间内 ${data.total.calls} 次调用的平均值`}
            />
          </div>

          {/* 子块一：走势（与左栏的收支图占同一个位置、同一个高度） */}
          <div className="p-3">
            <div className="mb-1.5 flex flex-wrap items-center gap-2">
              <h4 className="text-xs font-semibold tracking-wide text-fg2">Token 消耗趋势</h4>
              <span className="flex-1" />
              <span className="text-xs text-muted">
                {data.scanned_at ? `采集 ${fmtTime(data.scanned_at)}` : '尚未采集'}
              </span>
            </div>
            <TrendLines days={data.days} />
          </div>

          {/* 子块二：按软件与模型拆分（与左栏「各软件消耗」占同一个位置、同一个高度） */}
          <div className="p-3">
            <div className="mb-1 flex items-baseline justify-between gap-2 px-3 text-xs text-muted">
              <span>软件</span>
              <span>Token · 占比</span>
            </div>
            <RankedAppUsage byApp={data.by_app} totalTokens={data.total.tokens} />
            <div className="mb-1 mt-2 flex items-baseline justify-between gap-2 border-t border-line/70 px-3 pt-2 text-xs text-muted">
              <span>模型</span>
              <span>Token · 占比</span>
            </div>
            <RankedModelUsage byModel={data.by_model} totalTokens={data.total.tokens} />
          </div>
        </>
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
