// 仪表盘 = 健康中心 + 积分 + 用量，一页看完「有没有事要处理」「钱进出了多少」「Token 花在哪」。
//
// 三块都读本地库，只有刷新积分/扫描文件/跑一轮这几个动作会打平台接口。
// 顺序按「要不要动手」排：账号结论 → 积分收支（含按软件拆分）→ 用量统计。
// 运行细节（存储占用、定时任务）与领取明细分别归设置页与日志页，这里不再重复。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Button, cn } from '@/components/ui/button';
import { Input, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { fmtAmount } from '@/lib/format';
import { errText } from '@/lib/ipc';
import { CreditFlowChart } from '@/components/credit-flow';
import { CreditPlatformSplit } from '@/components/credit-platforms';
import { CreditTrend } from '@/components/credit-trend';
import type { AccountView, AppConfig, CreditFlowReport, HealthReport, TrendPoint } from '@/lib/types';
import { Metric, SectionTitle } from './metric';
import { UsageSection } from './usage-section';

const NAV_EVENT = 'aim:navigate';

/** 收支图最多回看 60 天：后端 `credit_flow` 就是这个上限，UI 不承诺更多。 */
const MAX_RANGE_DAYS = 60;

/** 跨能力页跳转。账号中心挂载时消费 aim.focusAccount 并选中对应行。 */
function go(id: string, iid?: string) {
  if (iid) sessionStorage.setItem('aim.focusAccount', iid);
  else sessionStorage.removeItem('aim.focusAccount');
  window.dispatchEvent(new CustomEvent(NAV_EVENT, { detail: id }));
}

export default function OverviewPage() {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const [days, setDays] = useState(30);
  // 两种视角共存：收支看「进出」，余额看「存量走势」，默认收支
  const [chartMode, setChartMode] = useState<'flow' | 'balance'>('flow');
  // 自定义区间：起止日期由原生 date 控件给出（YYYY-MM-DD），与后端按日聚合的口径一致
  const [customOpen, setCustomOpen] = useState(false);
  const [customFrom, setCustomFrom] = useState('');
  const [customTo, setCustomTo] = useState('');
  const [bundlePath, setBundlePath] = useState<string | null>(null);

  const health = useQuery<HealthReport>({
    queryKey: ['health'],
    queryFn: () => queryCmd('system_health'),
    // 窗口先于后端 State 就绪创建（实测首行日志比窗口晚 ~5s），启动瞬间的首次
    // 调用可能失败；有界重试 1 次（6s 后）让竞态自愈，结构性错误也只多记 1 条。
    retry: 1,
    retryDelay: 6_000,
    // 出错后退避到 5 分钟再试：错误即永久停轮会让仪表盘整块数据冻成启动快照
    // （v1.9.1 九类测试实测：数据库停在采集前大小、日志 0 B 达 4 分钟以上）。
    refetchInterval: (query) => (query.state.error ? 300_000 : 60_000),
  });
  const accounts = useQuery<AccountView[]>({
    queryKey: ['accounts'],
    queryFn: () => queryCmd('account_list'),
  });
  // 自定义区间的跨度 → 向后端要多长的时间窗（后端按「最近 N 天」取数，再在前端裁掉区间外的点）
  const spanDays = useMemo(() => {
    if (!customOpen || !customFrom) return days;
    const from = new Date(`${customFrom}T00:00:00`);
    const to = new Date(`${customTo || todayIso()}T00:00:00`);
    if (Number.isNaN(from.getTime()) || Number.isNaN(to.getTime())) return days;
    const span = Math.round((to.getTime() - from.getTime()) / 86_400_000) + 1;
    return Math.min(MAX_RANGE_DAYS, Math.max(1, span));
  }, [customOpen, customFrom, customTo, days]);
  const flow = useQuery<CreditFlowReport>({
    queryKey: ['flow', spanDays],
    queryFn: () => queryCmd('credit_flow', { days: spanDays }),
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5 * 60_000),
  });
  // 余额趋势只在切到「余额」时才查，别让默认视图多打一次数据库
  const trend = useQuery<TrendPoint[]>({
    queryKey: ['trend', spanDays],
    queryFn: () => queryCmd('credit_trend', { days: spanDays }),
    enabled: chartMode === 'balance',
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5 * 60_000),
  });
  // 区间外的点必须裁掉：后端只认「最近 N 天」，自定义的止日期可能早于今天
  const flowPoints = useMemo(
    () =>
      (flow.data?.total ?? []).filter((point) =>
        inRange(point.date, customOpen, customFrom, customTo),
      ),
    [flow.data, customOpen, customFrom, customTo],
  );
  // 每个软件同样先裁点再交给表格：合计数由点现算，裁剪后才和上图对得上
  const platformFlows = useMemo(
    () =>
      (flow.data?.platforms ?? []).map((row) => ({
        ...row,
        points: row.points.filter((point) =>
          inRange(point.date, customOpen, customFrom, customTo),
        ),
      })),
    [flow.data, customOpen, customFrom, customTo],
  );
  const trendPoints = useMemo(
    () => (trend.data ?? []).filter((point) => inRange(point.date, customOpen, customFrom, customTo)),
    [trend.data, customOpen, customFrom, customTo],
  );

  /** 打开自定义区间时先给一个「最近 30 天」的默认值，避免两个空控件。 */
  const openCustom = () => {
    if (!customFrom || !customTo) {
      const to = new Date();
      setCustomTo(isoOf(to));
      setCustomFrom(isoOf(new Date(to.getTime() - 29 * 86_400_000)));
    }
    setCustomOpen(true);
  };
  // 补刷阈值来自应用配置（设置界面可调）；配置没到时用默认值兜底
  const config = useQuery<AppConfig>({
    queryKey: ['config'],
    queryFn: () => queryCmd('config_get'),
  });
  const staleAfterMs = (config.data?.dashboard.stale_refresh_min ?? 10) * 60 * 1000;

  const report = health.data;
  const rows = accounts.data ?? [];

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

  const autoRefreshing = useRef(false);
  const lastAutoTry = useRef(0);
  const [refreshing, setRefreshing] = useState(false);

  // 积分不是实时接口：超过 10 分钟就自动刷一轮，否则「已用掉的积分」会一直挂在告警里
  useEffect(() => {
    if (!report || autoRefreshing.current) return;
    const at = report.credits_at;
    const stale = at === null || Date.now() - at > staleAfterMs;
    if (!stale || Date.now() - lastAutoTry.current < 5 * 60 * 1000) return;
    const iids = rows.filter((row) => row.has_secret && row.enabled).map((row) => row.iid);
    if (iids.length === 0) return;
    autoRefreshing.current = true;
    lastAutoTry.current = Date.now();
    setRefreshing(true);
    void (async () => {
      try {
        // 自动刷新失败不弹 toast：错误会体现在「积分数据 N 分钟前」上，不打断用户；
        // trigger=auto 与手动按钮区分，动作日志里显示「自动」而不是「手动」
        await runCmd('checkin_credits', { iids, trigger: 'auto' }, { onError: () => {} });
        await Promise.all([
          qc.invalidateQueries({ queryKey: ['accounts'] }),
          qc.invalidateQueries({ queryKey: ['health'] }),
          qc.invalidateQueries({ queryKey: ['flow'] }),
        ]);
      } finally {
        autoRefreshing.current = false;
        setRefreshing(false);
      }
    })();
  }, [report, rows, qc, staleAfterMs]);

  const refreshCredits = () =>
    run('credits', async () => {
      const iids = rows.filter((row) => row.has_secret && row.enabled).map((row) => row.iid);
      if (iids.length === 0) {
        toast('没有可刷新的账号');
        return;
      }
      await runCmd('checkin_credits', { iids });
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['accounts'] }),
        qc.invalidateQueries({ queryKey: ['health'] }),
          qc.invalidateQueries({ queryKey: ['flow'] }),
      ]);
      toast('积分已刷新');
    });

  const runRound = () =>
    run('round', async () => {
      if ((await runCmd('daily_run_now')) === undefined) return;
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['accounts'] }),
        qc.invalidateQueries({ queryKey: ['detail'] }),
        qc.invalidateQueries({ queryKey: ['health'] }),
      ]);
    });

  const batchDiagnose = () =>
    run('diagnose', async () => {
      const iids = rows.filter((r) => r.has_secret && r.enabled).map((r) => r.iid);
      if (iids.length === 0) {
        toast('没有可诊断的账号（需要凭据且已启用）');
        return;
      }
      const result = await runCmd('account_batch_diagnose', { iids });
      if (result === undefined) return;
      const head = `批量诊断：${result.total} 个账号 · 成功 ${result.ok} · 失败 ${result.failed.length}`;
      toast(
        result.failed.length === 0
          ? head
          : `${head}\n${result.failed.slice(0, 2).join('\n')}${
              result.failed.length > 2 ? `\n…另 ${result.failed.length - 2} 条见日志` : ''
            }`,
      );
      await qc.invalidateQueries({ queryKey: ['accounts'] });
    });

  const exportBundle = () =>
    run('bundle', async () => {
      const path = await runCmd('diagnose_bundle');
      if (path === undefined) return;
      setBundlePath(path);
      toast(`诊断包已导出：${path}`);
    });

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <h1 className="text-lg font-semibold">仪表盘</h1>
        {report && (
          <span className="text-xs text-muted">
            积分数据 {ago(report.credits_at)}
            {refreshing && ' · 正在刷新…'}
          </span>
        )}
        <span className="flex-1" />
        <Button
          size="sm"
          variant="ghost"
          onClick={refreshCredits}
          loading={busy === 'credits'}
          disabled={!!busy || refreshing}
          title="重新查一遍各账号的积分与批次（会调用平台接口）"
        >
          刷新积分
        </Button>
        <Button size="sm" variant="ghost" onClick={() => health.refetch()} disabled={!!busy}>
          刷新
        </Button>
        <Button
          size="sm"
          onClick={batchDiagnose}
          loading={busy === 'diagnose'}
          disabled={!!busy || rows.length === 0}
          title="对全部启用且有凭据的账号做一次只读诊断（并发 3）"
        >
          批量诊断
        </Button>
        <Button
          size="sm"
          onClick={exportBundle}
          loading={busy === 'bundle'}
          disabled={!!busy}
          title="把日志、账号状态（脱敏）、配置打包成一个 JSON，便于排查或求助"
        >
          导出诊断包
        </Button>
        <Button
          size="sm"
          variant="primary"
          onClick={runRound}
          loading={busy === 'round'}
          disabled={!!busy}
          title="立即跑一轮签到轮询（会真的调用平台接口）"
        >
          跑一轮
        </Button>
      </div>

      {bundlePath && (
        <div className="flex items-center gap-2 rounded-md border border-line bg-card px-3 py-1.5 text-xs">
          <span className="text-muted">最近导出</span>
          <span className="truncate font-mono" title={bundlePath}>
            {bundlePath}
          </span>
          <span className="flex-1" />
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              void navigator.clipboard.writeText(bundlePath);
              toast('路径已复制');
            }}
          >
            复制路径
          </Button>
        </div>
      )}

      {health.isLoading && (
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          {Array.from({ length: 4 }).map((_, index) => (
            <div key={index} className="h-[92px] animate-pulse rounded-lg bg-hover" />
          ))}
        </div>
      )}

      {health.isError && (
        <div className="rounded-lg border border-line bg-card">
          <StateBlock
            kind="error"
            title="读健康报告失败"
            detail={errText(health.error)}
            onRetry={() => health.refetch()}
          />
        </div>
      )}

      {report && report.accounts.total === 0 && (
        <div className="rounded-lg border border-dashed border-line">
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
      )}

      {report && report.accounts.total > 0 && (
        <>
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
            <Metric
              label="启用账号"
              value={`${report.accounts.enabled} / ${report.accounts.total}`}
              hint={`停用 ${report.accounts.total - report.accounts.enabled} · 无凭据 ${report.accounts.no_secret}`}
              onClick={() => go('account-center')}
            />
            <Metric
              label="今日已签"
              value={`${report.accounts.checked_today}`}
              hint={
                report.accounts.checked_today === 0
                  ? '还没有账号完成签到'
                  : `覆盖 ${report.accounts.checked_today} 个账号`
              }
              tone={report.accounts.checked_today > 0 ? 'ok' : undefined}
              onClick={() => go('account-center')}
            />
            <Metric
              label="到期风险"
              value={`${report.accounts.expired + report.accounts.expiring}`}
              hint={`已过期 ${report.accounts.expired} · 3 天内 ${report.accounts.expiring}`}
              tone={
                report.accounts.expired > 0
                  ? 'bad'
                  : report.accounts.expiring > 0
                    ? 'warn'
                    : undefined
              }
              onClick={() => go('account-center')}
            />
            <Metric
              label="最近失败"
              value={`${report.accounts.failing}`}
              hint={
                report.accounts.failing > 0
                  ? '失败晚于最近一次成功，需要看一眼'
                  : '全部账号最近一次查询正常'
              }
              tone={report.accounts.failing > 0 ? 'bad' : 'ok'}
              onClick={() => go('account-center')}
            />
          </div>

          <section className="rounded-lg border border-line bg-card p-3">
            <SectionTitle
              right={
                <span className="flex flex-wrap items-center gap-1">
                  <span className="mr-1 flex items-center gap-0.5 rounded border border-line p-0.5">
                    {(['flow', 'balance'] as const).map((mode) => (
                      <button
                        key={mode}
                        className={cn(
                          'cursor-pointer rounded px-1.5 py-0.5',
                          chartMode === mode ? 'bg-hover text-fg' : 'text-muted hover:text-fg',
                        )}
                        onClick={() => setChartMode(mode)}
                        title={
                          mode === 'flow'
                            ? '按天看进账与消耗（双向柱）'
                            : '按天看余额存量走势（折线）'
                        }
                      >
                        {mode === 'flow' ? '收支' : '余额'}
                      </button>
                    ))}
                  </span>
                  {[7, 30, 60].map((value) => (
                    <button
                      key={value}
                      className={cn(
                        'cursor-pointer rounded px-1.5 py-0.5',
                        !customOpen && days === value
                          ? 'bg-hover text-fg'
                          : 'text-muted hover:text-fg',
                      )}
                      onClick={() => {
                        setCustomOpen(false);
                        setDays(value);
                      }}
                    >
                      {value} 天
                    </button>
                  ))}
                  <button
                    className={cn(
                      'cursor-pointer rounded px-1.5 py-0.5',
                      customOpen ? 'bg-hover text-fg' : 'text-muted hover:text-fg',
                    )}
                    onClick={openCustom}
                  >
                    自定义
                  </button>
                </span>
              }
            >
              积分{chartMode === 'flow' ? '收支' : '趋势'}（全部账号合计）
            </SectionTitle>

            {customOpen && (
              <div className="mb-2 flex flex-wrap items-center gap-2 text-xs">
                <label className="flex items-center gap-1 text-muted">
                  起
                  <Input
                    type="date"
                    className="w-36 text-xs"
                    value={customFrom}
                    max={customTo || todayIso()}
                    onChange={(e) => setCustomFrom(e.target.value)}
                  />
                </label>
                <label className="flex items-center gap-1 text-muted">
                  止
                  <Input
                    type="date"
                    className="w-36 text-xs"
                    value={customTo}
                    min={customFrom || undefined}
                    max={todayIso()}
                    onChange={(e) => setCustomTo(e.target.value)}
                  />
                </label>
                <span className="text-muted">
                  最多回看 {MAX_RANGE_DAYS} 天
                  {customFrom && customTo && customFrom > customTo ? ' · 起日期晚于止日期' : ''}
                </span>
              </div>
            )}

            {chartMode === 'flow' ? (
              flow.isLoading ? (
                <div className="h-[200px] animate-pulse rounded-md bg-hover" />
              ) : (
                <CreditFlowChart points={flowPoints} />
              )
            ) : trend.isLoading ? (
              <div className="h-[200px] animate-pulse rounded-md bg-hover" />
            ) : (
              <CreditTrend points={trendPoints} />
            )}
            <div className="mt-1 flex items-center gap-2 text-xs text-muted">
              <span>
                7 天内到期{' '}
                <span className={expiringCredit > 0 ? 'font-medium text-warn' : ''}>
                  {fmtAmount(expiringCredit)}
                </span>{' '}
                积分
              </span>
              <span className="flex-1" />
              <button
                className="cursor-pointer hover:text-fg"
                onClick={() => go('account-center')}
              >
                去用掉 →
              </button>
            </div>
          </section>

          {/* 第二层：同样一份账目按软件拆开，看「钱花在哪」 */}
          {flow.isLoading ? (
            <div className="h-[180px] animate-pulse rounded-lg bg-hover" />
          ) : (
            <CreditPlatformSplit
              platforms={platformFlows}
              note={
                customOpen && customFrom
                  ? `${customFrom} ~ ${customTo || todayIso()}`
                  : `最近 ${days} 天`
              }
            />
          )}
        </>
      )}

      {/* 用量与账号无关：没有账号也要能看本机客户端的 Token 消耗 */}
      <UsageSection />
    </div>
  );
}

/** 「3 分钟前」这类相对时间：积分数据的陈旧程度比绝对时间更直观。 */
function ago(ms: number | null): string {
  if (ms === null) return '从未刷新';
  const diff = Date.now() - ms;
  if (diff < 60_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  return `${Math.floor(diff / 86_400_000)} 天前`;
}

/** 自定义区间裁剪：`open` 为假时原样放行（预设区间由后端按 days 截好）。 */
function inRange(date: string, open: boolean, from: string, to: string): boolean {
  if (!open) return true;
  return (!from || date >= from) && (!to || date <= to);
}

/** 本地日期 → `YYYY-MM-DD`：date 控件与后端按日聚合都用这个口径，不做时区换算。 */
function isoOf(date: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${p(date.getMonth() + 1)}-${p(date.getDate())}`;
}

function todayIso(): string {
  return isoOf(new Date());
}
