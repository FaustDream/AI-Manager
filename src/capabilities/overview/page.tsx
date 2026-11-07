// 仪表盘 = 健康中心：先回答「有没有事要处理」，再回答「现在什么状态」。
//
// 三块信息按「要不要动手」排序：待处理告警 → 账号/积分结论 → 运行细节
// （趋势、定时任务、存储、平台最近结果）。全部读本地，不打平台接口。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Button, cn } from '@/components/ui/button';
import { StateBlock } from '@/components/ui/form';
import { Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { fmtAmount, fmtSize, fmtTime } from '@/lib/format';
import { errText } from '@/lib/ipc';
import { CreditTrend } from '@/components/credit-trend';
import { CreditFlowChart } from '@/components/credit-flow';
import type { AccountView, CreditFlowPoint, HealthReport, RecentClaim, UpdateInfo } from '@/lib/types';

const NAV_EVENT = 'aim:navigate';

/** 跨能力页跳转。账号中心挂载时消费 aim.focusAccount 并选中对应行。 */
function go(id: string, iid?: string) {
  if (iid) sessionStorage.setItem('aim.focusAccount', iid);
  else sessionStorage.removeItem('aim.focusAccount');
  window.dispatchEvent(new CustomEvent(NAV_EVENT, { detail: id }));
}

function Metric({
  label,
  value,
  hint,
  tone,
  onClick,
}: {
  label: string;
  value: string;
  hint: string;
  tone?: 'ok' | 'warn' | 'bad';
  onClick?: () => void;
}) {
  return (
    <div
      role={onClick ? 'button' : undefined}
      tabIndex={onClick ? 0 : undefined}
      onClick={onClick}
      onKeyDown={(e) => {
        if (onClick && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault();
          onClick();
        }
      }}
      className={cn(
        'rounded-lg border border-line bg-card p-3',
        onClick && 'cursor-pointer transition-colors hover:border-accent/60 hover:bg-hover',
      )}
    >
      <div className="text-xs text-muted">{label}</div>
      <div
        className={cn(
          'mt-1 text-2xl font-semibold tabular-nums',
          tone === 'ok' && 'text-ok',
          tone === 'warn' && 'text-warn',
          tone === 'bad' && 'text-bad',
        )}
      >
        {value}
      </div>
      <div className="mt-0.5 truncate text-xs text-muted" title={hint}>
        {hint}
      </div>
    </div>
  );
}

function SectionTitle({ children, right }: { children: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div className="mb-1.5 flex items-baseline gap-2">
      <h3 className="text-xs font-semibold tracking-wide text-fg2">{children}</h3>
      <span className="flex-1" />
      {right && <span className="text-xs text-muted">{right}</span>}
    </div>
  );
}

export default function OverviewPage() {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const [days, setDays] = useState(30);
  const [chartMode, setChartMode] = useState<'balance' | 'flow'>('balance');
  const [bundlePath, setBundlePath] = useState<string | null>(null);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);

  const health = useQuery<HealthReport>({
    queryKey: ['health'],
    queryFn: () => queryCmd('system_health'),
    // 命令不存在 / 读库失败属于结构性错误：继续轮询只会每分钟刷一条同样的错误
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 60_000),
  });
  const trend = useQuery({
    queryKey: ['trend', days],
    queryFn: () => queryCmd('credit_trend', { days }),
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5 * 60_000),
  });
  const accounts = useQuery<AccountView[]>({
    queryKey: ['accounts'],
    queryFn: () => queryCmd('account_list'),
  });
  const flow = useQuery<CreditFlowPoint[]>({
    queryKey: ['flow', days],
    queryFn: () => queryCmd('credit_flow', { days }),
    enabled: chartMode === 'flow',
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5 * 60_000),
  });
  const claims = useQuery<RecentClaim[]>({
    queryKey: ['recent-claims'],
    queryFn: () => queryCmd('credit_claims_recent', { limit: 20 }),
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 60_000),
  });

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
    const stale = at === null || Date.now() - at > 10 * 60 * 1000;
    if (!stale || Date.now() - lastAutoTry.current < 5 * 60 * 1000) return;
    const iids = rows.filter((row) => row.has_secret && row.enabled).map((row) => row.iid);
    if (iids.length === 0) return;
    autoRefreshing.current = true;
    lastAutoTry.current = Date.now();
    setRefreshing(true);
    void (async () => {
      try {
        // 自动刷新失败不弹 toast：错误会体现在「积分数据 N 分钟前」上，不打断用户
        await runCmd('checkin_credits', { iids }, { onError: () => {} });
        await Promise.all([
          qc.invalidateQueries({ queryKey: ['accounts'] }),
          qc.invalidateQueries({ queryKey: ['health'] }),
          qc.invalidateQueries({ queryKey: ['trend'] }),
          qc.invalidateQueries({ queryKey: ['flow'] }),
          qc.invalidateQueries({ queryKey: ['recent-claims'] }),
        ]);
      } finally {
        autoRefreshing.current = false;
        setRefreshing(false);
      }
    })();
  }, [report, rows, qc]);

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
        qc.invalidateQueries({ queryKey: ['trend'] }),
          qc.invalidateQueries({ queryKey: ['flow'] }),
        qc.invalidateQueries({ queryKey: ['recent-claims'] }),
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

  const checkUpdate = () =>
    run('update', async () => {
      const info = await runCmd('update_check');
      if (info === undefined) return;
      setUpdate(info);
    });

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <h1 className="text-lg font-semibold">仪表盘</h1>
        {report && (
          <span className="text-xs text-muted">
            v{report.version} · 积分数据 {ago(report.credits_at)}
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

      {update && (
        <div className="flex flex-wrap items-center gap-2 rounded-md border border-line bg-card px-3 py-1.5 text-xs">
          {update.has_update ? (
            <span className="text-warn">
              有新版本 v{update.latest}（当前 v{update.current}）
            </span>
          ) : (
            <span className="text-ok">已是最新版本 v{update.current}</span>
          )}
          <span className="flex-1" />
          <Button
            size="sm"
            variant="ghost"
            onClick={() => runCmd('open_external', { url: update.url })}
          >
            打开发布页
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

          <div className="grid gap-3 lg:grid-cols-[minmax(0,1fr)_320px]">
            <section className="rounded-lg border border-line bg-card p-3">
              <SectionTitle
                right={
                  <span className="flex items-center gap-1">
                    <span className="mr-1 flex items-center gap-0.5 rounded border border-line p-0.5">
                      {(['balance', 'flow'] as const).map((mode) => (
                        <button
                          key={mode}
                          className={cn(
                            'cursor-pointer rounded px-1.5 py-0.5',
                            chartMode === mode ? 'bg-hover text-fg' : 'text-muted hover:text-fg',
                          )}
                          onClick={() => setChartMode(mode)}
                        >
                          {mode === 'balance' ? '余额' : '收支'}
                        </button>
                      ))}
                    </span>
                    {[7, 30, 60].map((value) => (
                      <button
                        key={value}
                        className={cn(
                          'cursor-pointer rounded px-1.5 py-0.5',
                          days === value ? 'bg-hover text-fg' : 'text-muted hover:text-fg',
                        )}
                        onClick={() => setDays(value)}
                      >
                        {value} 天
                      </button>
                    ))}
                  </span>
                }
              >
                积分{chartMode === 'balance' ? '趋势' : '收支'}（全部账号合计）
              </SectionTitle>
              {chartMode === 'balance' ? (
                trend.isLoading ? (
                  <div className="h-[180px] animate-pulse rounded-md bg-hover" />
                ) : (
                  <CreditTrend points={trend.data ?? []} />
                )
              ) : flow.isLoading ? (
                <div className="h-[180px] animate-pulse rounded-md bg-hover" />
              ) : (
                <CreditFlowChart points={flow.data ?? []} />
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

            <section className="rounded-lg border border-line bg-card p-3">
              <SectionTitle>运行与存储</SectionTitle>
              {/* minmax(0,1fr)：数据目录长路径时 truncate 才能真正截断，而不是把侧栏撑宽 */}
              <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
                <dt className="text-muted">数据目录</dt>
                <dd className="truncate font-mono" title={report.data_dir}>
                  {report.data_dir}
                </dd>
                <dt className="text-muted">数据库</dt>
                <dd className="tabular-nums">{fmtSize(report.db_bytes)}</dd>
                <dt className="text-muted">日志</dt>
                <dd className="tabular-nums">{fmtSize(report.log_bytes)}</dd>
                <dt className="text-muted">备份</dt>
                <dd className="tabular-nums">{fmtSize(report.backup_bytes)}</dd>
                <dt className="text-muted">平台</dt>
                <dd className="flex flex-wrap gap-1">
                  {report.platforms.map((platform) => (
                    <span
                      key={platform.platform}
                      className={cn(
                        'rounded px-1',
                        platform.failing > 0 ? 'bg-bad/15 text-bad' : 'bg-hover text-fg2',
                      )}
                      title={`最近成功：${fmtTime(platform.last_ok_at)} · 最近失败：${fmtTime(platform.last_fail_at)}`}
                    >
                      {platform.label} {platform.accounts}
                      {platform.failing > 0 ? ` · 失败 ${platform.failing}` : ''}
                    </span>
                  ))}
                </dd>
              </dl>
              <div className="mt-2 flex items-center gap-2">
                <Button size="sm" variant="ghost" onClick={checkUpdate} loading={busy === 'update'}>
                  检查更新
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => runCmd('maintenance_open_data')}
                >
                  打开数据目录
                </Button>
              </div>
            </section>
          </div>

          <section className="rounded-lg border border-line bg-card p-3">
            <SectionTitle right="最近 8 条 · 仅本工具实际领取">最近领取</SectionTitle>
            {claims.isLoading && <div className="h-16 animate-pulse rounded bg-hover" />}
            {!claims.isLoading && (claims.data ?? []).length === 0 && (
              <p className="text-xs text-muted">
                还没有领取记录。签到成功、领取活动或猫猫旅行领奖后，这里会出现明细。
              </p>
            )}
            {(claims.data ?? []).length > 0 && (
              <table className="w-full border-collapse text-xs">
                <thead>
                  <tr className="text-left text-muted">
                    <th className="w-36 py-1 font-medium">时间</th>
                    <th className="w-40 py-1 font-medium">账号</th>
                    <th className="w-20 py-1 font-medium">来源</th>
                    <th className="w-20 py-1 text-right font-medium">积分</th>
                    <th className="py-1 font-medium">说明</th>
                  </tr>
                </thead>
                <tbody>
                  {(claims.data ?? []).slice(0, 8).map((claim) => (
                    <tr
                      key={`${claim.iid}-${claim.claimed_at}-${claim.description}`}
                      className="cursor-pointer border-t border-line/60 hover:bg-hover"
                      onClick={() => go('account-center', claim.iid)}
                    >
                      <td className="py-1 tabular-nums text-muted">{fmtTime(claim.claimed_at)}</td>
                      <td className="truncate py-1">{claim.name}</td>
                      <td className="py-1">
                        <Tag tone={claim.source === 'travel' ? 'ok' : 'muted'}>
                          {claim.source === 'checkin'
                            ? '签到'
                            : claim.source === 'travel'
                              ? '猫猫旅行'
                              : claim.source === 'activity'
                                ? '活动'
                                : claim.source}
                        </Tag>
                      </td>
                      <td className="py-1 text-right tabular-nums">
                        {claim.amount === null ? '—' : `+${fmtAmount(claim.amount)}`}
                      </td>
                      <td className="max-w-[320px] truncate py-1 text-fg2" title={claim.description}>
                        {claim.description}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </section>

          <section className="rounded-lg border border-line bg-card p-3">
            <SectionTitle right={`${report.schedules.length} 条`}>定时任务</SectionTitle>
            {report.schedules.length === 0 ? (
              <p className="text-xs text-muted">还没有定时任务，去设置页添加。</p>
            ) : (
              <table className="w-full border-collapse text-xs">
                <thead>
                  <tr className="text-left text-muted">
                    <th className="py-1 font-medium">名称</th>
                    <th className="py-1 font-medium">规则</th>
                    <th className="py-1 font-medium">上次执行</th>
                    <th className="py-1 font-medium">下次执行</th>
                    <th className="py-1 text-right font-medium">状态</th>
                  </tr>
                </thead>
                <tbody>
                  {report.schedules.map((item) => (
                    <tr key={item.id} className="border-t border-line/60">
                      <td className="py-1">
                        {item.name}
                        <span className="ml-1 text-muted">{item.action}</span>
                      </td>
                      <td className="py-1 text-muted">{item.rule}</td>
                      <td className="py-1 tabular-nums">
                        {item.last_run ? fmtTime(item.last_run) : '从未运行'}
                      </td>
                      <td className="py-1 tabular-nums text-muted">{fmtTime(item.next_run)}</td>
                      <td className="py-1 text-right">
                        <Tag tone={!item.enabled ? 'muted' : item.last_run ? 'ok' : 'warn'}>
                          {!item.enabled ? '已停用' : item.last_run ? '正常' : '待首次运行'}
                        </Tag>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </section>
        </>
      )}
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
