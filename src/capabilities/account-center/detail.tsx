// 详情面板：表格下方常驻，选中行即换。只读本地库（不打接口），动作按钮单发。
//
// 版式按「先结论、后明细」排：指标卡只回答「还剩多少 / 什么快过期」，
// 往下才是额度窗口、积分批次与账号元信息。切换/删除在列表操作列，
// 这里只留这个账号自己的动作（签到、领取活动）。
// 「今日增减」「下次可签」两张卡已删：前者要看两天快照才有值、多数时候是空的，
// 后者在签到列和头部标签里已经答过一遍。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useState } from 'react';
import type { ReactNode } from 'react';

import { daysUntil, fmtAmount, fmtDay, fmtTime } from '@/lib/format';
import { Button, cn } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Notice, Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import type { AccountDetail, AccountView, ClaimPlan } from '@/lib/types';

function Stat({
  label,
  value,
  hint,
  tone,
  strong,
}: {
  label: string;
  value: string;
  hint?: string;
  tone?: 'ok' | 'warn' | 'bad';
  strong?: boolean;
}) {
  return (
    <div className="min-w-0 bg-card px-3 py-2.5">
      <div className="text-xs text-muted">{label}</div>
      <div
        className={cn(
          'mt-0.5 truncate tabular-nums',
          strong ? 'text-lg font-semibold' : 'text-md font-medium',
          tone === 'ok' && 'text-ok',
          tone === 'warn' && 'text-warn',
          tone === 'bad' && 'text-bad',
        )}
        title={value}
      >
        {value}
      </div>
      {hint && <div className="mt-0.5 truncate text-xs text-muted" title={hint}>{hint}</div>}
    </div>
  );
}

function SectionTitle({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="mb-1.5 flex items-baseline gap-2">
      <h3 className="text-xs font-semibold tracking-wide text-fg2">{children}</h3>
      <span className="flex-1" />
      {right && <span className="text-xs text-muted">{right}</span>}
    </div>
  );
}

export function DetailPanel({
  account,
  isCurrent,
  error,
}: {
  account: AccountView | null;
  isCurrent: boolean;
  error?: string;
}) {
  const qc = useQueryClient();
  const [claimPlans, setClaimPlans] = useState<ClaimPlan[] | null>(null);
  // 慢操作（签到/拉活动列表都是几秒级的网络+写盘）的按钮加载态
  const { busy, run: runBusy } = useBusy();

  const detail = useQuery<AccountDetail>({
    queryKey: ['detail', account?.iid],
    queryFn: () => queryCmd('checkin_detail', { iid: account!.iid }),
    enabled: !!account,
  });

  if (!account) {
    return (
      <div className="rounded-lg border border-dashed border-line px-4 py-5 text-sm text-muted">
        选中一行看详情：余额、额度窗口、积分批次与凭据状态。
      </div>
    );
  }

  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['accounts'] });
    qc.invalidateQueries({ queryKey: ['detail', account.iid] });
  };

  const runOne = () =>
    runBusy('checkin', async () => {
      
        const r = await runCmd('checkin_run', { iid: account.iid });

        if (r === undefined) return;
        toast(r.message);
        refresh();
      
    });

  const openClaim = () =>
    runBusy('plans', async () => {
      
        const plans = await runCmd('zcode_claim_plans', { iid: account.iid });

        if (plans === undefined) return;
        setClaimPlans(plans);
      
    });

  const claim = (planId: string, name: string) =>
    runBusy('claim', async () => {
      setClaimPlans(null);
      toast(`已打开浏览器，请在页面里完成验证…`);
      
        const r = await runCmd('zcode_claim_run', { iid: account.iid, planId });

        if (r === undefined) return;
        toast(`领取成功：${r.name || name}`);
        refresh();
      
    });

  const d = detail.data;
  const checked = d?.checked_in ?? account.checked_in === true;
  const now = Date.now();

  // 批次：按到期日从近到远；没有到期日的排最后
  const lots = [...(d?.lots ?? [])].sort((a, b) =>
    (a.expires_at ?? '9999').localeCompare(b.expires_at ?? '9999'),
  );
  const nearest = lots.find((lot) => lot.expires_at && new Date(lot.expires_at).getTime() > now);
  const nearestDays = daysUntil(nearest?.expires_at);
  const quota = Object.entries(d?.quota ?? account.quota).sort((a, b) => b[1] - a[1]);
  // 只有额度型平台（ZCode）的「更新时间」说的是配额；其余平台别拿批次缓存时刻冒充
  const quotaAt =
    account.platform === 'zcode' ? (d?.quota_at ?? account.quota_at) : null;
  const lastError = error ?? account.error;
  const credDays = daysUntil(account.expires_at);
  // 额度型账号没有积分余额：主指标直接给最紧窗口的剩余百分比，别只显示「—」
  const tightest = quota.length > 0 ? Math.max(...quota.map(([, used]) => used)) : null;
  const quotaLeft = tightest === null ? null : Math.max(0, 100 - tightest);

  return (
    <div className="overflow-hidden rounded-lg border border-line bg-card">
      {/* 头部：身份 + 这个账号自己的动作 */}
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-2">
        <h2 className="text-md font-semibold">{account.name}</h2>
        <Tag>{account.platform_label}</Tag>
        {isCurrent && <Tag tone="ok">当前登录</Tag>}
        {account.requires_checkin && (
          <Tag tone={checked ? 'ok' : 'warn'}>{checked ? '今日已签' : '今日未签'}</Tag>
        )}
        {!account.has_secret && <Tag tone="warn">无凭据</Tag>}
        <span className="flex-1" />
        {account.platform === 'zcode' && (
          <Button size="sm" onClick={openClaim} loading={busy === 'plans'} disabled={!!busy}>
            领取活动
          </Button>
        )}
        {account.requires_checkin && (
          <Button
            size="sm"
            variant="primary"
            onClick={runOne}
            loading={busy === 'checkin'}
            disabled={!!busy || !account.has_secret}
          >
            签到
          </Button>
        )}
      </div>

      {/* 结论层：只留「现在还剩多少」和「什么快过期」，数字最大，异常才上色 */}
      <div className="grid grid-cols-2 gap-px bg-line">
        <Stat
          label={quota.length > 0 ? '积分 / 配额' : '积分余额'}
          value={
            quotaLeft !== null && (d?.balance ?? account.balance) === null
              ? `剩 ${Math.round(quotaLeft)}%`
              : fmtAmount(d?.balance ?? account.balance)
          }
          hint={quota.length > 0 ? '额度型账号，明细见下方窗口' : '平台实时返回'}
          tone={quotaLeft !== null && quotaLeft <= 10 ? 'bad' : quotaLeft !== null && quotaLeft <= 30 ? 'warn' : undefined}
          strong
        />
        <Stat
          label="最近到期"
          value={nearest?.expires_at ? `${fmtDay(nearest.expires_at)} · ${fmtAmount(nearest.amount)}` : '—'}
          hint={
            nearest
              ? nearestDays !== null && nearestDays <= 7
                ? `只剩 ${Math.max(0, nearestDays)} 天，优先用掉`
                : '积分批次里最近的一笔'
              : '平台没有返回批次到期时间'
          }
          tone={nearestDays !== null && nearestDays <= 7 ? 'warn' : undefined}
        />
      </div>

      {/* 额度窗口：只有额度型平台有 */}
      {quota.length > 0 && (
        <section className="border-t border-line px-3 py-2.5">
          <SectionTitle right={quotaAt ? `更新于 ${fmtTime(quotaAt)}` : undefined}>额度窗口</SectionTitle>
          <div className="grid gap-x-6 gap-y-2 md:grid-cols-2">
            {quota.map(([name, used]) => {
              const left = Math.max(0, 100 - used);
              const color =
                left <= 10 ? 'var(--color-bad)' : left <= 30 ? 'var(--color-warn)' : 'var(--color-accent)';
              return (
                <div key={name} className="min-w-0">
                  <div className="mb-1 flex items-baseline gap-2 text-xs">
                    <span className="truncate text-fg2" title={name}>{name}</span>
                    <span className="flex-1" />
                    <span className={cn('tabular-nums', left <= 10 ? 'text-bad' : left <= 30 ? 'text-warn' : 'text-muted')}>
                      已用 {used.toFixed(0)}% · 剩 {left.toFixed(0)}%
                    </span>
                  </div>
                  <div className="h-1.5 overflow-hidden rounded-full bg-hover">
                    <div className="h-full rounded-full" style={{ width: `${Math.min(100, used)}%`, background: color }} />
                  </div>
                </div>
              );
            })}
          </div>
        </section>
      )}
      {quota.length === 0 && account.platform === 'zcode' && (
        <section className="border-t border-line px-3 py-2.5 text-xs text-muted">
          这个账号没有额度明细（Free 套餐或接口未下发）。
          {quotaAt ? `上次刷新 ${fmtTime(quotaAt)}。` : '点上方「刷新积分」拉一次。'}
        </section>
      )}

      {/* 明细层：左侧批次，右侧账号信息；宽屏并排，窄屏自动堆叠 */}
      <div className="grid border-t border-line lg:grid-cols-[minmax(0,1fr)_360px]">
        <section className="px-3 py-2.5">
          <SectionTitle right={lots.length > 0 ? `共 ${lots.length} 笔` : undefined}>积分批次</SectionTitle>
        {lots.length === 0 ? (
          <p className="text-xs text-muted">
            还没有批次明细：签到成功后这里会列出每笔的金额、来源与到期日。
          </p>
        ) : (
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="text-left text-xs text-muted">
                <th className="w-20 py-1 font-medium">到期</th>
                <th className="w-20 py-1 font-medium">金额</th>
                <th className="py-1 font-medium">类型 / 发放</th>
                <th className="w-24 py-1 text-right font-medium">状态</th>
              </tr>
            </thead>
            <tbody>
              {lots.slice(0, 6).map((lot, index) => {
                const days = daysUntil(lot.expires_at);
                const tone = days === null ? 'muted' : days < 0 ? 'bad' : days <= 7 ? 'warn' : 'muted';
                const text =
                  days === null ? '长期' : days < 0 ? '已过期' : days === 0 ? '今天到期' : `剩 ${days} 天`;
                return (
                  <tr key={`${lot.lot_key || lot.label}-${index}`} className="border-t border-line/60">
                    <td className="py-1 tabular-nums">{fmtDay(lot.expires_at)}</td>
                    <td className="py-1 tabular-nums">{fmtAmount(lot.amount)}</td>
                    <td className="truncate py-1">
                      <span>{lot.label || (lot.lot_key ? '签到发放' : '平台批次')}</span>
                      {lot.granted_at && (
                        <span className="ml-2 text-xs text-muted">{fmtDay(lot.granted_at)} 发放</span>
                      )}
                    </td>
                    <td className="py-1 text-right">
                      <Tag tone={tone}>{text}</Tag>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
        {lots.length > 6 && (
          <p className="mt-1 text-xs text-muted">另有 {lots.length - 6} 笔未显示（按到期日排序）。</p>
        )}
        </section>

        {/* 元信息：解释每一列的数据从哪来 */}
        <section className="border-t border-line px-3 py-2.5 lg:border-l lg:border-t-0">
          <SectionTitle>账号信息</SectionTitle>
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-xs">
            <dt className="text-muted">平台</dt>
            <dd>{account.platform_label}</dd>
            <dt className="text-muted">凭据</dt>
            <dd className="truncate" title={account.source || undefined}>
              {account.has_secret ? `已保存${account.source ? `（${account.source}）` : ''}` : '未保存'}
            </dd>
            <dt className="text-muted">凭据到期</dt>
            <dd
              className={cn(
                credDays !== null && credDays < 0 && 'text-bad',
                credDays !== null && credDays >= 0 && credDays <= 3 && 'text-warn',
              )}
            >
              {account.expires_at
                ? `${fmtDay(account.expires_at)}${
                    credDays === null
                      ? ''
                      : credDays < 0
                        ? ` · 已过期 ${Math.abs(credDays)} 天`
                        : ` · 剩 ${credDays} 天`
                  }`
                : '平台未提供'}
            </dd>
            <dt className="text-muted">配额更新</dt>
            <dd>{quotaAt ? fmtTime(quotaAt) : account.platform === 'zcode' ? '还没刷过' : '—'}</dd>
            <dt className="text-muted">恢复对话</dt>
            <dd>
              {account.platform === 'workbuddy'
                ? isCurrent
                  ? '可用（当前账号）'
                  : '切换到该账号后可用'
                : '不适用'}
            </dd>
            <dt className="text-muted">最近查询</dt>
            <dd className={cn(lastError && 'text-bad')}>{lastError ? '失败' : '正常'}</dd>
          </dl>
        </section>
      </div>

      {lastError && (
        <div className="px-3 pb-3">
          <Notice tone="bad">最近一次查询失败：{lastError}</Notice>
        </div>
      )}

      <Dialog open={claimPlans !== null} title="可领取的活动" onClose={() => setClaimPlans(null)} wide>
        {(claimPlans ?? []).length === 0 ? (
          <p className="text-sm text-muted">当前没有可领取的活动。</p>
        ) : (
          <div className="flex flex-col gap-2">
            {(claimPlans ?? []).map((plan) => (
              <div key={plan.plan_id} className="rounded-md border border-line p-2 text-sm">
                <div className="flex items-center gap-2">
                  <span className="font-semibold">{plan.name || plan.plan_id}</span>
                  {plan.campaign && <Tag tone="warn">{plan.campaign.title}</Tag>}
                  <span className="flex-1" />
                  <Button size="sm" variant="primary" onClick={() => claim(plan.plan_id, plan.name)}>
                    领取
                  </Button>
                </div>
                {plan.description && <div className="text-xs text-muted">{plan.description}</div>}
                {plan.grants.length > 0 && <div className="mt-1 text-xs">{plan.grants.join('；')}</div>}
              </div>
            ))}
          </div>
        )}
      </Dialog>
    </div>
  );
}
