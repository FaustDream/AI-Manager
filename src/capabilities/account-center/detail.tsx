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
import { Input, Textarea } from '@/components/ui/form';
import type { ReactNode } from 'react';

import { daysUntil, fmtAmount, fmtDay, fmtMs, fmtTime } from '@/lib/format';
import { Button, cn } from '@/components/ui/button';
import { confirmAction } from '@/components/ui/confirm';
import { Dialog } from '@/components/ui/dialog';
import { Notice, Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import type {
  AccountDetail,
  AccountView,
  ClaimPlan,
  CodebuddyTargetStatus,
  RewardCredits,
} from '@/lib/types';
import { DailyCard } from './daily';
import { TravelCard } from './travel';
import { ReloginDialog } from './toolbar';
import { DataTable, type Column } from '@/components/ui/table';
import type { CreditClaim } from '@/lib/types';

const claimSources: Record<string, string> = { checkin: '签到', activity: '活动', travel: '猫猫旅行' };
const claimColumns: Column<CreditClaim>[] = [
  { key: 'date', header: '日期', width: 160, cell: (row) => fmtTime(row.claimed_at) },
  { key: 'source', header: '来源', width: 100, cell: (row) => claimSources[row.source] ?? row.source },
  { key: 'amount', header: '积分', width: 100, align: 'right', cell: (row) => row.amount == null ? '未提供' : fmtAmount(row.amount) },
  { key: 'description', header: '描述', cell: (row) => row.description },
];

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
  const [aliasOpen, setAliasOpen] = useState(false);
  const [aliasDraft, setAliasDraft] = useState('');
  const [tagOpen, setTagOpen] = useState(false);
  const [tagDraft, setTagDraft] = useState('');
  const [noteOpen, setNoteOpen] = useState(false);
  const [noteDraft, setNoteDraft] = useState('');
  const [reloginOpen, setReloginOpen] = useState(false);
  // 平台奖励积分明细：与「积分批次」不同，它按包名聚合、含已用完之外的完整口径
  const [rewardOpen, setRewardOpen] = useState(false);
  // 慢操作（签到/拉活动列表都是几秒级的网络+写盘）的按钮加载态
  const { busy, run: runBusy } = useBusy();

  const detail = useQuery<AccountDetail>({
    queryKey: ['detail', account?.iid],
    queryFn: () => queryCmd('checkin_detail', { iid: account!.iid }),
    enabled: !!account,
    // 详情只读本地库，代价很低；面板开着就定时重读一次，
    // 后台签到/旅行写入即使漏了事件，领取记录也会很快自己出现。
    refetchInterval: 15_000,
  });

  // CodeBuddy 软件 / IDEA 插件现在登的是谁——切换前后都给个明确状态。
  // 这两个客户端只服务国内版：国际版账号连状态都不查（查了也没有可切的东西）。
  const codebuddyTargets = useQuery<CodebuddyTargetStatus[]>({
    queryKey: ['codebuddy-targets'],
    queryFn: () => queryCmd('account_codebuddy_targets'),
    enabled: !!account && account.platform === 'workbuddy' && !account.iid.endsWith('@intl'),
  });

  // 平台奖励积分明细：只在弹窗打开时拉一次，避免每点开一个账号就打一次接口
  const reward = useQuery<RewardCredits>({
    queryKey: ['reward-credits', account?.iid],
    queryFn: () => queryCmd('account_reward_credits', { iid: account!.iid }),
    enabled: !!account && rewardOpen,
    retry: false,
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

  const diagnose = () =>
    runBusy('diagnose', async () => {
      const report = await runCmd('checkin_diagnose', { iid: account.iid });
      if (report === undefined) return;
      toast(`诊断完成：${report.summary || '无返回'} · ${fmtMs(report.ms)}`);
      refresh();
    });

  const openAlias = () => {
    setAliasDraft(account.alias ?? '');
    setAliasOpen(true);
  };

  const openTags = () => {
    setTagDraft(account.tags.join('、'));
    setTagOpen(true);
  };

  const openNote = () => {
    setNoteDraft(account.note ?? '');
    setNoteOpen(true);
  };

  const saveTags = () =>
    runBusy('tags', async () => {
      // 中英文逗号、顿号、空白都当分隔符：用户不会记得「标准」写法
      const tags = tagDraft
        .split(/[，,、\s]+/)
        .map((tag) => tag.trim())
        .filter(Boolean);
      const saved = await runCmd('account_set_tags', { iid: account.iid, tags });
      if (saved === undefined) return;
      setTagOpen(false);
      toast(saved.length > 0 ? `标签已保存：${saved.join('、')}` : '标签已清空');
      qc.invalidateQueries({ queryKey: ['accounts'] });
    });

  const saveNote = () =>
    runBusy('note', async () => {
      const saved = await runCmd('account_set_note', { iid: account.iid, note: noteDraft });
      if (saved === undefined) return;
      setNoteOpen(false);
      toast(saved ? '备注已保存' : '备注已清空');
      qc.invalidateQueries({ queryKey: ['accounts'] });
    });

  const saveAlias = () =>
    runBusy('alias', async () => {
      const saved = await runCmd('account_set_alias', {
        iid: account.iid,
        alias: aliasDraft,
      });
      if (saved === undefined) return;
      setAliasOpen(false);
      toast(saved ? `别名已保存：${saved}` : '已恢复平台昵称');
      qc.invalidateQueries({ queryKey: ['accounts'] });
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

  // 三端同号、分开切换：这两个按钮只动 CodeBuddy 软件 / IDEA 插件的登录，
  // 用这个账号的 WorkBuddy 档案写入（列表上的「切换登录」只切 WorkBuddy 本体）。
  // 只对国内版开放：CodeBuddy 系客户端没有国际版授权域，切过去也登不上。
  const canSwitchCodebuddy =
    account.platform === 'workbuddy' && !account.iid.endsWith('@intl');
  const switchCodebuddy = (target: 'software' | 'idea') =>
    runBusy(`switch-${target}`, async () => {
      const label = target === 'software' ? 'CodeBuddy 软件' : 'IDEA 插件';
      const ok = await confirmAction({
        title: `切换 ${label} 登录`,
        message:
          `把 ${label} 的登录切成国内版账号「${account.name}」？\n\n` +
          `用这个账号的 WorkBuddy 档案写入，写前自动备份；需要先关闭 ${label}。\n` +
          '切换后会清掉该端的账号缓存，模型与权限按新账号重新拉取。',
      });
      if (!ok) return;
      const r = await runCmd('account_switch_codebuddy', { iid: account.iid, target });
      if (r === undefined) return;
      toast(r.message);
      qc.invalidateQueries({ queryKey: ['codebuddy-targets'] });
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
      {/* 头部：身份 + 这个账号自己的动作。名称可到 40 字（别名），压不掉就截断，
          不能把右侧动作行顶出面板 */}
      <div className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-2">
        <h2 className="min-w-0 truncate text-md font-semibold" title={account.name}>
          {account.name}
        </h2>
        <Tag>{account.platform_label}</Tag>
        {isCurrent && <Tag tone="ok">当前登录</Tag>}
        {account.requires_checkin && (
          <Tag tone={checked ? 'ok' : 'warn'}>{checked ? '今日已签' : '今日未签'}</Tag>
        )}
        {!account.has_secret && <Tag tone="warn">无凭据</Tag>}
        <span className="flex-1" />
        <Button
          size="sm"
          variant="ghost"
          onClick={openAlias}
          disabled={!!busy}
          title="设置本机显示名；别名单独保存，重扫客户端不会覆盖"
        >
          改名
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={openTags}
          disabled={!!busy}
          title="标签用于筛选与批量操作，最多 8 个"
        >
          标签
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={openNote}
          disabled={!!busy}
          title="备注只存本机，用来记「为什么停用」「绑定在哪台机器」"
        >
          备注
        </Button>
        {/* 凭据缺失的账号切不过去：这里给它补一次登录，成功后凭据自动落库 */}
        {account.platform !== 'trae' && (
          <Button
            size="sm"
            variant={account.has_secret ? 'ghost' : 'primary'}
            onClick={() => setReloginOpen(true)}
            disabled={!!busy}
            title={
              account.has_secret
                ? '重新走一次平台登录，成功后覆盖已保存的凭据与到期时间'
                : '这个账号没有可用凭据，切不过去；点这里重新登录，成功后自动保存'
            }
          >
            重新登录
          </Button>
        )}
        <Button
          size="sm"
          variant="ghost"
          onClick={diagnose}
          loading={busy === 'diagnose'}
          disabled={!!busy || !account.has_secret}
          title="只读诊断：调用平台查询接口，不写库、不触发签到；结果同时进日志"
        >
          诊断
        </Button>
        {account.platform === 'zcode' && (
          <Button
            size="sm"
            onClick={openClaim}
            loading={busy === 'plans'}
            disabled={!!busy}
            title="拉取当前可领的活动套餐；领取时会打开浏览器过验证码。也可以在设置里开自动领取"
          >
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
        {canSwitchCodebuddy && (
          <>
            <Button
              size="sm"
              onClick={() => switchCodebuddy('software')}
              loading={busy === 'switch-software'}
              disabled={!!busy}
              title="把 CodeBuddy 软件的登录切成这个账号"
            >
              切换 CodeBuddy 软件
            </Button>
            <Button
              size="sm"
              onClick={() => switchCodebuddy('idea')}
              loading={busy === 'switch-idea'}
              disabled={!!busy}
              title="把 IDEA 里 CodeBuddy 插件的登录切成这个账号"
            >
              切换 IDEA 插件
            </Button>
          </>
        )}
      </div>

      {/* 切换状态行：两个客户端现在登的是谁 / 有没有装，一眼看清 */}
      {canSwitchCodebuddy && (
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-line px-3 py-1.5 text-xs text-muted">
          <span className="font-medium">CodeBuddy 系登录</span>
          {codebuddyTargets.isLoading && <span>读取中…</span>}
          {(codebuddyTargets.data ?? []).map((t) => (
            <span key={t.key}>
              {t.label}：
              {!t.installed ? (
                <span className="text-muted">未安装</span>
              ) : t.current_name ? (
                <span className="font-medium text-fg">
                  {t.current_name}
                  {t.edition === 'intl' ? '（国际版）' : t.edition === 'cn' ? '（国内版）' : ''}
                </span>
              ) : (
                <span className="text-muted">未登录</span>
              )}
            </span>
          ))}
        </div>
      )}

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

      {/* 猫猫旅行与每日活跃都只属于 WorkBuddy 国内版；国际版没有这两项 */}
      {account.platform === 'workbuddy' && !account.iid.endsWith('@intl') && account.has_secret && (
        <>
          <TravelCard account={account} />
          <DailyCard account={account} />
        </>
      )}

      <section className="border-t border-line px-3 py-2.5">
        <SectionTitle right="最近 100 条 · 签到 / 活动 / 猫猫旅行">积分领取记录</SectionTitle>
        <div className="max-h-72 overflow-auto">
          <DataTable
            columns={claimColumns}
            className="min-w-lg"
            rows={d?.claims ?? []}
            rowKey={(row) => row.event_key}
            loading={detail.isLoading}
            dense
            stickyHead
            empty={<p className="py-3 text-xs text-muted">暂无领取记录；余额快照、旧批次和已领取状态不会补记。</p>}
          />
        </div>
      </section>

      {/* 明细层：左侧批次，右侧账号信息；宽屏并排，窄屏自动堆叠 */}
      <div className="grid border-t border-line lg:grid-cols-[minmax(0,1fr)_360px]">
        <section className="px-3 py-2.5">
          <div className="mb-1.5 flex items-baseline gap-2">
            <h3 className="text-xs font-semibold tracking-wide text-fg2">积分批次</h3>
            <span className="flex-1" />
            {lots.length > 0 && <span className="text-xs text-muted">共 {lots.length} 笔</span>}
            {account.platform === 'workbuddy' && (
              <Button size="sm" variant="ghost" onClick={() => setRewardOpen(true)}>
                平台奖励积分
              </Button>
            )}
          </div>
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
          {/* 1fr 换成 minmax(0,1fr)：值列才有收缩余地，凭据长路径 truncate 生效、长备注不撑爆侧栏 */}
          <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
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
            <dt className="text-muted">标签</dt>
            <dd>{account.tags.length > 0 ? account.tags.join('、') : '未标记'}</dd>
            <dt className="text-muted">备注</dt>
            <dd className="break-words whitespace-pre-wrap">{account.note || '—'}</dd>
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

      <Dialog
        open={aliasOpen}
        title="设置账号别名"
        onClose={() => setAliasOpen(false)}
        footer={
          <>
            <Button onClick={() => setAliasOpen(false)}>取消</Button>
            <Button variant="primary" onClick={saveAlias} loading={busy === 'alias'}>
              保存
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-2">
          <label className="flex flex-col gap-1.5">
            <span className="text-xs text-muted">显示名</span>
            <Input
              value={aliasDraft}
              maxLength={40}
              placeholder={account.name}
              onChange={(e) => setAliasDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void saveAlias();
              }}
            />
          </label>
          <p className="text-xs text-muted">
            只存在本机账号库，不写客户端，也不会在重扫时被平台昵称覆盖。留空即恢复平台昵称。
          </p>
        </div>
      </Dialog>

      <Dialog
        open={tagOpen}
        title="设置账号标签"
        onClose={() => setTagOpen(false)}
        footer={
          <>
            <Button onClick={() => setTagOpen(false)}>取消</Button>
            <Button variant="primary" onClick={saveTags} loading={busy === 'tags'}>
              保存
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-2">
          <label className="flex flex-col gap-1.5">
            <span className="text-xs text-muted">标签（用逗号、顿号或空格分隔）</span>
            <Input
              value={tagDraft}
              placeholder="工作号、待处理"
              maxLength={140}
              onChange={(e) => setTagDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void saveTags();
              }}
            />
          </label>
          <p className="text-xs text-muted">
            最多 8 个，单个标签不超过 16 字。只存本机，重扫客户端不会覆盖；列表顶部可按标签筛选。
          </p>
        </div>
      </Dialog>

      <Dialog
        open={noteOpen}
        title="账号备注"
        onClose={() => setNoteOpen(false)}
        footer={
          <>
            <Button onClick={() => setNoteOpen(false)}>取消</Button>
            <Button variant="primary" onClick={saveNote} loading={busy === 'note'}>
              保存
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-2">
          <Textarea
            value={noteDraft}
            maxLength={200}
            placeholder="例如：这个号只在公司电脑用；停用是因为额度给同事了"
            onChange={(e) => setNoteDraft(e.target.value)}
          />
          <p className="text-xs text-muted">最多 200 字，只存本机。留空即清空。</p>
        </div>
      </Dialog>

      <Dialog
        open={rewardOpen}
        title="平台奖励积分"
        onClose={() => setRewardOpen(false)}
        wide
      >
        {reward.isLoading && <p className="text-xs text-muted">正在读取平台明细…</p>}
        {reward.isError && (
          <p className="text-xs text-bad">读取失败：{String(reward.error)}</p>
        )}
        {reward.data && (
          <div className="flex flex-col gap-2 text-sm">
            <div className="flex items-baseline gap-2">
              <span className="text-xs text-muted">平台口径总积分</span>
              <span className="text-lg font-semibold tabular-nums">
                {fmtAmount(reward.data.total)}
              </span>
              <span className="flex-1" />
              <span className="text-xs text-muted">
                还有余额的 {reward.data.packages.length} 个来源包
              </span>
            </div>
            {reward.data.packages.length === 0 ? (
              <p className="text-xs text-muted">平台没有返回还有余额的资源包。</p>
            ) : (
              <table className="w-full border-collapse text-xs">
                <thead>
                  <tr className="text-left text-muted">
                    <th className="py-1 font-medium">来源包</th>
                    <th className="py-1 text-right font-medium">剩余 / 总量</th>
                    <th className="py-1 text-right font-medium">批次</th>
                    <th className="py-1 text-right font-medium">最晚到期</th>
                  </tr>
                </thead>
                <tbody>
                  {reward.data.packages.map((pkg) => (
                    <tr key={pkg.name} className="border-t border-line/60">
                      <td className="py-1">{pkg.name}</td>
                      <td className="py-1 text-right tabular-nums">
                        {fmtAmount(pkg.remain)} / {fmtAmount(pkg.size)}
                      </td>
                      <td className="py-1 text-right tabular-nums">{pkg.count}</td>
                      <td className="py-1 text-right tabular-nums">
                        {pkg.expires_at ? fmtDay(pkg.expires_at) : '—'}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        )}
      </Dialog>

      <ReloginDialog
        target={reloginOpen ? account : null}
        onClose={() => setReloginOpen(false)}
        onDone={refresh}
      />

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
