// 仪表盘 = 积分 + 用量，一页看完「钱进出了多少」「Token 花在哪」。
//
// 版式：左右两栏各占一半，两张卡**同构**——标题行（区间控件）+ 2×2 数字瓦片 + 两个子块
// （左：收支图 / 各软件消耗；右：Token 趋势 / 软件·模型排名）。同构是为了对齐：
// 两栏行高由较高一侧撑起，卡片上下边界因此落在同一条线上。会话区占整页一行。
// 窄屏（<xl）自动回落到上下堆叠。
//
// 默认区间 7 天（不是今天）：当天还没刷新积分时收支常常整排是 0，
// 一进页面看到四个 0 会以为功能坏了；7 天既有形状又能看出趋势。
//
// 区间只有一份，放在页头做「全局过滤」（'./range'）：积分、Token、会话三块共用同一个
// 时间窗——同一次进页面就该是同一个口径，三份控件各调各的只会让两张卡上的数字互相对不上。
// 不设回看上限，起止日期直接交给后端。
//
// 两块都读本地库，只有补刷积分这个动作会打平台接口（Token 采集由后台定时器负责）。
// 账号健康摘要（启用/已签/到期/失败）与页头动作条已撤：这些动作账号中心、
// 顶栏与命令面板里都有，结论以账号中心为准，这里不再重复一份。
// 运行细节（存储占用、定时任务）与领取明细分别归设置页与日志页。
// 首启的「只在本机运行」安全说明也已撤：它属于一次性说明，不该常驻占位。
import { useQuery } from '@tanstack/react-query';
import { ArrowRight } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { StateBlock } from '@/components/ui/form';
import { queryCmd } from '@/lib/cmd';
import { fmtAmount } from '@/lib/format';
import { errText } from '@/lib/ipc';
import { CreditFlowChart } from '@/components/credit-flow';
import { CreditPlatformSplit } from '@/components/credit-platforms';
import { CreditTrend } from '@/components/credit-trend';
import type { AccountView, CreditFlowReport, HealthReport, TrendPoint } from '@/lib/types';
import { Metric, SectionTitle, Segmented } from './metric';
import { RangeControl, useRange } from './range';
import { SessionsSection } from './sessions-section';
import { UsageSection, useUsageQuery } from './usage-section';

const NAV_EVENT = 'aim:navigate';

/** 跨能力页跳转。账号中心挂载时消费 aim.focusAccount 并选中对应行。 */
function go(id: string, iid?: string) {
  if (iid) sessionStorage.setItem('aim.focusAccount', iid);
  else sessionStorage.removeItem('aim.focusAccount');
  window.dispatchEvent(new CustomEvent(NAV_EVENT, { detail: id }));
}

export default function OverviewPage() {
  // 默认看最近 7 天：当天还没刷新积分时收支常常是 0，一排 0 会被读成「坏了」
  // 一份区间喂给三块：积分 / Token / 会话（页头的「全局过滤」）
  const range = useRange(7);
  const usage = useUsageQuery(range);
  // 两种视角共存：收支看「进出」，余额看「存量走势」，默认收支
  const [chartMode, setChartMode] = useState<'flow' | 'balance'>('flow');

  const health = useQuery<HealthReport>({
    queryKey: ['health'],
    queryFn: () => queryCmd('system_health'),
    // 窗口先于后端 State 就绪创建（实测首行日志比窗口晚 ~5s），启动瞬间的首次
    // 调用可能失败；有界重试 1 次（6s 后）让竞态自愈，结构性错误也只多记 1 条。
    retry: 1,
    retryDelay: 6_000,
    // 出错后退避到 5 分钟再试：错误即永久停轮会让仪表盘整块数据冻成启动快照
    // （数据库停在采集前大小、日志 0 B 达 4 分钟以上）。
    refetchInterval: (query) => (query.state.error ? 300_000 : 60_000),
  });
  const accounts = useQuery<AccountView[]>({
    queryKey: ['accounts'],
    queryFn: () => queryCmd('account_list'),
  });
  const flow = useQuery<CreditFlowReport>({
    queryKey: ['flow', range.from, range.to],
    queryFn: () => queryCmd('credit_flow', { from: range.from, to: range.to }),
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5 * 60_000),
  });
  // 余额趋势只在切到「余额」时才查，别让默认视图多打一次数据库
  const trend = useQuery<TrendPoint[]>({
    queryKey: ['trend', range.from, range.to],
    queryFn: () => queryCmd('credit_trend', { from: range.from, to: range.to }),
    enabled: chartMode === 'balance',
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5 * 60_000),
  });
  // 后端按 from/to 取数，点已经在区间内，前端不再裁
  const flowPoints = flow.data?.total ?? [];
  // 区间内没有快照、或一分没进也没花的软件都不列——「0」会被读成「没花钱」，
  // 而真实情况往往是「没数据」或「今天还没刷新过积分」。
  const platformFlows = useMemo(
    () =>
      (flow.data?.platforms ?? []).filter(
        (row) =>
          row.points.length > 0 &&
          row.points.some((point) => point.earned > 0 || point.spent > 0),
      ),
    [flow.data],
  );
  const trendPoints = trend.data ?? [];

  const report = health.data;
  const rows = accounts.data ?? [];

  // KPI 摘要条：区间进账 / 区间消耗 / 当前余额 / 凭据临期。
  // 前两项跟随所选区间（与下图同一份点，数字必然对得上）；
  // 余额是存量、临期是账号事实，都与区间无关。
  const earnedSum = useMemo(
    () => flowPoints.reduce((sum, point) => sum + point.earned, 0),
    [flowPoints],
  );
  const spentSum = useMemo(
    () => flowPoints.reduce((sum, point) => sum + point.spent, 0),
    [flowPoints],
  );
  const balanceSum = useMemo(
    () => rows.reduce((sum, row) => sum + (row.balance ?? 0), 0),
    [rows],
  );
  // 区间说明统一口径：KPI 标签、图表小标题、各软件消耗右上角都用 range.label
  const rangeText = range.label;

  // 7 天内到期的积分合计：告警里逐账号提醒，这里给一个总数，决定「要不要现在去用掉」
  const expiringCredit = useMemo(() => {
    const now = Date.now();
    return rows.reduce((sum, row) => {
      const at = row.lot_expires_at;
      const amount = row.lot_amount ?? 0;
      const balance = row.balance ?? 0;
      // 余额已经清零：批次缓存还没刷新，但那笔积分实际已经不在了
      if (
        at !== null &&
        amount > 0 &&
        balance > 0 &&
        at > now &&
        at - now <= 7 * 24 * 60 * 60 * 1000
      ) {
        // 批次金额大于当前余额时按余额封顶，和告警口径保持一致
        return sum + Math.min(amount, balance);
      }
      return sum;
    }, 0);
  }, [rows]);

  // 左栏的两种「替代卡」：健康报告失败 / 还没账号。任一成立就整栏只放这一张卡
  // （不占满行高），右栏的用量照常显示——它与账号无关。
  const creditPlaceholder = health.isError ? (
    <div>
      <StateBlock
        kind="error"
        title="读健康报告失败"
        detail={errText(health.error)}
        onRetry={() => health.refetch()}
      />
    </div>
  ) : report && report.accounts.total === 0 ? (
    <div>
      <StateBlock
        kind="empty"
        title="还没有账号"
        detail="启动时会自动扫描客户端；也可以去账号中心用浏览器登录添加。"
      />
      <div className="pb-6 text-center">
        <Button size="sm" variant="primary" onClick={() => go('account-center')}>
          去账号中心
        </Button>
      </div>
    </div>
  ) : null;

  return (
    <div className="stagger relative flex flex-col gap-6">
      {/* 极光底纹（叙事页专属）：只在仪表盘顶部铺一层极淡的慢渐变，
          表格页不挂——读表时底色在动会分神。不拦截指针。
          外层负责裁切：底纹本身要放大位移，不裁的话会给 main 撑出一条横向滚动条。
          负 z 让它落在卡片与文字之下，只染画布。 */}
      <div aria-hidden className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[460px] overflow-hidden">
        <div className="aurora-glow" />
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2">
        {/* 全局过滤：三块（积分 / Token / 会话）共用这一个区间。
            各卡自己再挂一份控件，等于同一个页面有三个时间口径。 */}
        <span
          className="flex flex-wrap items-center gap-1.5"
          role="group"
          aria-label="全局过滤"
          title="统一控制积分统计、Token 统计与会话统计的时间区间"
        >
          <span className="text-xs text-muted">全局过滤</span>
          <RangeControl range={range} />
        </span>
      </div>

      {/* 两栏：左积分、右用量。两张卡**同构**——标题行 + 2×2 数字 + 两个子块。
          对齐靠 subgrid：父栅格定出「标题 / 数字 / 子块一 / 子块二」四行，两张卡各跨满四行，
          行高取两侧较大值——于是两栏的子块边界也落在同一条水平线上，不用手调高度。
          窄屏（<xl）回落成上下堆叠（此时卡内是普通单列栅格）。 */}
      <div className="grid gap-5 xl:grid-cols-2 xl:grid-rows-[auto_auto_auto_auto]">
        {creditPlaceholder ? (
          <div className="min-w-0 self-start">{creditPlaceholder}</div>
        ) : (
          <Card
            className="grid min-w-0 gap-4 p-5 xl:row-span-4 xl:grid-rows-subgrid"
            aria-label="积分统计"
          >
            <div>
              {/* 区间控件已上移到页头「全局过滤」：这一行只留标题，但保留行高，
                  否则右栏的标题行会被 subgrid 拉高、两卡数字行对不齐 */}
              <SectionTitle className="mb-0 min-h-7">积分统计</SectionTitle>
            </div>

            {/* 四个数字回答「进了多少、花了多少、还剩多少、有没有要紧事」。
                半栏宽度下排成 2×2：四列会把每个数字挤到读不出来。 */}
            {flow.isLoading ? (
              <div className="grid grid-cols-2 gap-4">
                {Array.from({ length: 4 }).map((_, i) => (
                  <div key={i} className="h-[92px] skeleton-block rounded-lg bg-hover" />
                ))}
              </div>
            ) : (
              <div className="grid grid-cols-2 gap-4" role="group" aria-label="关键指标">
                <Metric
                  label={`进账 · ${rangeText}`}
                  value={fmtAmount(earnedSum)}
                  raw={earnedSum}
                  format={fmtAmount}
                  tone="ok"
                  hint="所选区间内全部账号收到的积分合计，与下方收支图同一口径"
                />
                <Metric
                  label={`消耗 · ${rangeText}`}
                  value={fmtAmount(spentSum)}
                  raw={spentSum}
                  format={fmtAmount}
                  hint="所选区间内全部账号用掉的积分合计，与下方收支图同一口径"
                />
                <Metric
                  label="当前余额"
                  value={fmtAmount(balanceSum)}
                  raw={balanceSum}
                  format={fmtAmount}
                  hint="全部账号最新余额合计（存量，不受区间影响）"
                />
                <Metric
                  label="凭据临期"
                  value={String(report?.accounts.expiring ?? 0)}
                  raw={report?.accounts.expiring ?? 0}
                  format={(n) => String(Math.round(n))}
                  tone={(report?.accounts.expiring ?? 0) > 0 ? 'warn' : 'default'}
                  hint="3 天内凭据到期的账号数：去账号中心刷新凭据或重新授权"
                  onClick={() => go('account-center')}
                />
              </div>
            )}

            {/* 子块一：按天看进账/消耗（或余额走势）。
                两栏行高由 subgrid 取较大值，于是左右子块边界落在同一条水平线上。 */}
            <div>
              <div className="mb-2 flex flex-wrap items-center gap-2">
                <h4 className="text-xs font-semibold tracking-wide text-muted">
                  积分{chartMode === 'flow' ? '收支' : '趋势'}
                </h4>
                <span className="flex-1" />
                <Segmented
                  value={chartMode}
                  options={[
                    { key: 'flow', label: '收支', title: '按天看进账与消耗（双向柱）' },
                    { key: 'balance', label: '余额', title: '按天看余额存量走势（折线）' },
                  ]}
                  onPick={(key) => setChartMode(key as 'flow' | 'balance')}
                />
                <span className="text-xs text-muted">{rangeText}</span>
              </div>

              {chartMode === 'flow' ? (
                flow.isLoading ? (
                  <div className="h-[240px] skeleton-block rounded-md bg-hover" />
                ) : (
                  <CreditFlowChart points={flowPoints} platforms={platformFlows} />
                )
              ) : trend.isLoading ? (
                <div className="h-[240px] skeleton-block rounded-md bg-hover" />
              ) : (
                <CreditTrend points={trendPoints} />
              )}

              <div className="mt-2 flex items-center gap-2 border-t border-line pt-2 text-xs text-muted">
                <span>
                  7 天内到期{' '}
                  <span className={expiringCredit > 0 ? 'font-medium text-warn' : ''}>
                    {fmtAmount(expiringCredit)}
                  </span>{' '}
                  积分
                </span>
                <span className="flex-1" />
                <button
                  type="button"
                  className="inline-flex cursor-pointer items-center gap-1 rounded-sm text-accent transition-colors duration-[var(--dur-1)] hover:text-accent-strong"
                  onClick={() => go('account-center')}
                >
                  去用掉
                  <ArrowRight size={13} />
                </button>
              </div>
            </div>

            {/* 子块二：同一份账目按软件拆开，看「钱花在哪」 */}
            <div className="border-t border-line pt-3">
              {flow.isLoading ? (
                <div className="h-[180px] skeleton-block rounded-md bg-hover" />
              ) : (
                <CreditPlatformSplit flush platforms={platformFlows} note={rangeText} />
              )}
            </div>
          </Card>
        )}

        <UsageSection query={usage} range={range} />
      </div>

      {/* 会话维度横跨整页：KPI 瓦片 + 来源/项目排名；区间同样来自页头的全局过滤 */}
      <SessionsSection range={range} />
    </div>
  );
}
