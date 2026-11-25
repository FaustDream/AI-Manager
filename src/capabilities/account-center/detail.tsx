// 账号详情弹窗：一个账号一块屏幕，按标签页拆成概览 / 积分批次 / 领取记录 / 猫猫旅行。
//
// 旧版把账号信息、指标、额度、两张明细表与旅行卡全铺在一个长面板里，越往下越难对号；
// 现在结论（概览）与账目（批次 / 领取 / 旅行）分开，切标签页不丢上下文。
// 只读本地库（不打接口），动作按钮单发；切换/删除仍在列表操作列。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { Input, Textarea } from '@/components/ui/form';
import { daysUntil, fmtAmount, fmtDay, fmtTime } from '@/lib/format';
import { Button, cn } from '@/components/ui/button';
import {
  IconCalendar,
  IconEdit,
  IconGift,
  IconLink,
  IconNotebook,
  IconRefresh,
} from '@/components/ui/icons';
import { confirmAction } from '@/components/ui/confirm';
import { Dialog } from '@/components/ui/dialog';
import { Hint, Notice, Tabs, Tag, Tooltip } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import type {
  AccountDetail,
  AccountView,
  ClaimPlan,
  CodebuddyTargetStatus,
  CreditClaim,
  CreditLot,
  RewardCredits,
  RewardPackage,
} from '@/lib/types';
import {
  FEATURE_CHECKIN,
  FEATURE_RESTORE,
  FEATURE_SWITCH_IDE,
  FEATURE_SWITCH_IDEA,
  FEATURE_TRAVEL,
  featurePlatform,
  usePlatformFeatures,
} from '@/lib/features';
import { TravelCard } from './travel';
import { ReauthDialog } from './toolbar';
import { creditUnit } from './credit-unit';
import { DataTable, type Column } from '@/components/ui/table';

const claimSources: Record<string, string> = { checkin: '签到', activity: '活动', travel: '猫猫旅行' };
const claimColumns = (unit: string): Column<CreditClaim>[] => [
  {
    key: 'date',
    header: '日期',
    width: 160,
    truncate: true,
    sortValue: (row) => row.claimed_at,
    cell: (row) => fmtTime(row.claimed_at),
  },
  {
    key: 'source',
    header: '来源',
    width: 100,
    truncate: true,
    title: (row) => claimSources[row.source] ?? row.source,
    sortValue: (row) => claimSources[row.source] ?? row.source,
    cell: (row) => claimSources[row.source] ?? row.source,
  },
  {
    key: 'amount',
    header: unit,
    width: 100,
    align: 'right',
    sortValue: (row) => row.amount,
    cell: (row) => (row.amount == null ? '未提供' : fmtAmount(row.amount)),
  },
  {
    key: 'description',
    header: '描述',
    truncate: true,
    title: (row) => row.description,
    sortValue: (row) => row.description,
    cell: (row) => row.description,
  },
];

/** 批次行加一层稳定 key：lot_key/label 可能重复，拼上原顺序保证唯一。 */
type LotRow = { lot: CreditLot; key: string };

const lotColumns: Column<LotRow>[] = [
  {
    key: 'expires',
    header: '到期',
    width: 112,
    className: 'tabular-nums',
    cell: (row) => fmtDay(row.lot.expires_at),
  },
  {
    key: 'amount',
    header: '金额',
    width: 112,
    className: 'tabular-nums',
    cell: (row) => fmtAmount(row.lot.amount),
  },
  {
    key: 'label',
    header: '类型 / 发放',
    truncate: true,
    title: (row) => row.lot.label || (row.lot.lot_key ? '签到发放' : '平台批次'),
    cell: (row) => (
      <span>
        {row.lot.label || (row.lot.lot_key ? '签到发放' : '平台批次')}
        {row.lot.granted_at && (
          <span className="ml-2 text-xs text-muted">{fmtDay(row.lot.granted_at)} 发放</span>
        )}
      </span>
    ),
  },
  {
    key: 'status',
    header: '状态',
    width: 128,
    align: 'right',
    cell: (row) => {
      const days = daysUntil(row.lot.expires_at);
      const tone = days === null ? 'muted' : days < 0 ? 'bad' : days <= 7 ? 'warn' : 'muted';
      const text =
        days === null
          ? '长期'
          : days < 0
            ? '已过期'
            : days === 0
              ? '今天到期'
              : `剩 ${days} 天`;
      return <Tag tone={tone}>{text}</Tag>;
    },
  },
];

const rewardColumns: Column<RewardPackage>[] = [
  {
    key: 'name',
    header: '来源包',
    truncate: true,
    title: (pkg) => pkg.name,
    className: 'max-w-[200px]',
    cell: (pkg) => pkg.name,
  },
  {
    key: 'remain',
    header: '剩余 / 总量',
    align: 'right',
    cell: (pkg) => `${fmtAmount(pkg.remain)} / ${fmtAmount(pkg.size)}`,
  },
  {
    key: 'count',
    header: '批次',
    align: 'right',
    cell: (pkg) => pkg.count,
  },
  {
    key: 'expires',
    header: '最晚到期',
    align: 'right',
    cell: (pkg) => (pkg.expires_at ? fmtDay(pkg.expires_at) : '—'),
  },
];

/** 剩余额度的告警档：≤10% 红、≤30% 黄、其余不告警。指标卡与额度条共用这一组阈值。 */
type QuotaTone = 'bad' | 'warn' | null;

function leftTone(left: number | null): QuotaTone {
  if (left === null) return null;
  return left <= 10 ? 'bad' : left <= 30 ? 'warn' : null;
}

/**
 * 额度利用率条：首次从 0 长到目标（延迟 80ms 让文字先落位），之后 used 变化直接过渡。
 * 刷新额度是低频动作，硬切的话用户看不出「这一条到底动没动」。
 * role=progressbar 让读屏能报出已用百分比，而不是只看到一行小字。
 */
function QuotaBar({ used, tone }: { used: number; tone: QuotaTone }) {
  const target = Math.min(100, used);
  const [pct, setPct] = useState(0);
  const mounted = useRef(false);
  useEffect(() => {
    if (mounted.current) {
      setPct(target);
      return;
    }
    mounted.current = true;
    const timer = window.setTimeout(() => setPct(target), 80);
    return () => window.clearTimeout(timer);
  }, [target]);
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(target)}
      aria-label="额度已用比例"
      className="h-1.5 overflow-hidden rounded-full bg-hover"
    >
      <div
        className={cn(
          'h-full rounded-full transition-[width,background-color] duration-[var(--dur-4)] ease-[var(--ease-std)]',
          // 有余量时用强调色：这条度量的是用量，不是告警
          tone === 'bad' ? 'bg-bad' : tone === 'warn' ? 'bg-warn' : 'bg-accent',
        )}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

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
  // 瓦片上只有 label + 数值；hint 是解释或建议，进 tooltip（dashboard-design：wordless）
  return (
    <div className="min-w-0 bg-card px-4 py-3">
      <div className="text-xs text-muted">
        <Hint content={hint} focusable={false}>
          {label}
        </Hint>
      </div>
      <Hint
        content={value}
        focusable={false}
        className={cn(
          'mt-0.5 block truncate tabular-nums',
          strong ? 'text-lg font-semibold' : 'text-md font-medium',
          tone === 'ok' && 'text-ok',
          tone === 'warn' && 'text-warn',
          tone === 'bad' && 'text-bad',
        )}
      >
        {value}
      </Hint>
    </div>
  );
}

function CardTitle({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="flex flex-1 flex-wrap items-baseline gap-2">
      <h3 className="text-xs font-semibold tracking-wide text-fg2">{children}</h3>
      <span className="flex-1" />
      {right && <span className="text-xs text-muted">{right}</span>}
    </div>
  );
}

type DetailTab = 'overview' | 'lots' | 'claims' | 'travel';

function DetailTabs({
  tab,
  items,
  onChange,
}: {
  tab: DetailTab;
  items: { key: DetailTab; label: string }[];
  onChange: (tab: DetailTab) => void;
}) {
  return (
    <Tabs
      label="账号详情分区"
      size="md"
      variant="solid"
      value={tab}
      className="self-start rounded-lg border border-line bg-card p-1 shadow-card"
      onValueChange={(next) => onChange(next as DetailTab)}
      items={items.map((item) => ({ value: item.key, label: item.label }))}
    />
  );
}

export function AccountDetailDialog({
  account,
  isCurrent,
  error,
  onClose,
}: {
  account: AccountView;
  isCurrent: boolean;
  error?: string;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const [tab, setTab] = useState<DetailTab>('overview');
  const [claimPlans, setClaimPlans] = useState<ClaimPlan[] | null>(null);
  const [aliasOpen, setAliasOpen] = useState(false);
  const [aliasDraft, setAliasDraft] = useState('');
  const [noteOpen, setNoteOpen] = useState(false);
  const [noteDraft, setNoteDraft] = useState('');
  const [reauthOpen, setReauthOpen] = useState(false);
  // 平台奖励积分明细：与「积分批次」不同，它按包名聚合、含已用完之外的完整口径
  const [rewardOpen, setRewardOpen] = useState(false);
  // 慢操作（签到/拉活动列表都是几秒级的网络+写盘）的按钮加载态
  const { busy, run: runBusy } = useBusy();
  // 哪个软件显示哪个入口由「设置 → 各软件功能」决定，这里不再写死平台名
  const features = usePlatformFeatures();

  // CodeBuddy 两个客户端各有一个开关：「切换 IDE」与「切换插件」在设置里分开配，
  // 能力表里也只有 WorkBuddy 国内版与 CodeBuddy 系写着支持（国际版没有授权域）。
  // 两个都关掉就连状态都不查——状态卡不渲染。
  const ideSwitchOn = features.on(featurePlatform(account.platform, account.iid), FEATURE_SWITCH_IDE);
  const ideaSwitchOn = features.on(
    featurePlatform(account.platform, account.iid),
    FEATURE_SWITCH_IDEA,
  );
  const canSwitchCodebuddy = ideSwitchOn || ideaSwitchOn;

  const detail = useQuery<AccountDetail>({
    queryKey: ['detail', account.iid],
    queryFn: () => queryCmd('checkin_detail', { iid: account.iid }),
    // 详情只读本地库，代价很低；弹窗开着就定时重读一次，
    // 后台签到/旅行写入即使漏了事件，领取记录也会很快自己出现。
    refetchInterval: 15_000,
  });

  // CodeBuddy IDE / CodeBuddy 插件现在登的是谁——切换前后都给个明确状态。
  // 两个入口都关掉就不查了：状态卡不渲染。
  const codebuddyTargets = useQuery<CodebuddyTargetStatus[]>({
    queryKey: ['codebuddy-targets'],
    queryFn: () => queryCmd('account_codebuddy_targets'),
    enabled: canSwitchCodebuddy,
  });

  // 平台奖励积分明细：只在弹窗打开时拉一次，避免每点开一个账号就打一次接口
  const reward = useQuery<RewardCredits>({
    queryKey: ['reward-credits', account.iid],
    queryFn: () => queryCmd('account_reward_credits', { iid: account.iid }),
    enabled: rewardOpen,
    retry: false,
  });

  // UID 从操作列搬到这里：它属于「点开这个账号才关心」的元信息，不占列表宽度。
  // 契约 17 仍然成立——列表不渲染 uid，只有详情弹窗打开时才按需取一次。
  const uid = useQuery<string>({
    queryKey: ['uid', account.iid],
    queryFn: () => queryCmd('account_uid', { iid: account.iid }),
    retry: false,
    staleTime: Infinity,
  });

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

  // 刷新账号：**不管当前是否过期都重扫一次**，把客户端里的最新凭据与有效期写回本地库。
  // 与列表上的「刷新」是同一个动作；判定由后端做（凭据真的变了才算刷新到新东西）。
  const refreshCredential = () =>
    runBusy('refresh-credential', async () => {
      const out = await runCmd('account_refresh_credential', { iid: account.iid });
      if (out === undefined) return;
      toast(out.message);
      refresh();
      // 重扫也会改变「当前登录」与可切换范围
      qc.invalidateQueries({ queryKey: ['current'] });
      qc.invalidateQueries({ queryKey: ['switchable'] });
    });

  const openAlias = () => {
    setAliasDraft(account.alias ?? '');
    setAliasOpen(true);
  };

  const openNote = () => {
    setNoteDraft(account.note ?? '');
    setNoteOpen(true);
  };

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

  // 三端同号、分开切换：这两个按钮只动 CodeBuddy IDE / CodeBuddy 插件的登录，
  // 用这个账号的 WorkBuddy 档案写入（列表上的「切换登录」只切 WorkBuddy 本体）。
  const switchCodebuddy = (target: 'software' | 'idea') =>
    runBusy(`switch-${target}`, async () => {
      const label = target === 'software' ? 'CodeBuddy IDE' : 'CodeBuddy 插件';
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

  // 切换按钮的置灰：对应的软件/插件没装（或用户没配对自定义目录）时不给点，
  // 点下去只会在报错里学一次。读不到状态按「装了」处理，避免状态还没拉到就闪灰。
  const codebuddyInstalled = (key: string) =>
    codebuddyTargets.data?.find((t) => t.key === key)?.installed ?? true;

  const d = detail.data;
  const checked = d?.checked_in ?? account.checked_in === true;
  const now = Date.now();
  // 豆包只做只读积分查询：平台回的是累计额度，没有批次也没有到期日
  const isDoubao = account.platform === 'doubao';

  // 批次：按到期日从近到远；没有到期日的排最后
  const lots = [...(d?.lots ?? [])].sort((a, b) =>
    (a.expires_at ?? '9999').localeCompare(b.expires_at ?? '9999'),
  );
  const nearest = lots.find((lot) => lot.expires_at && new Date(lot.expires_at).getTime() > now);
  const nearestDays = daysUntil(nearest?.expires_at);
  // 10 天内到期：与「最近到期」共用同一份批次，只算还没过期的
  const soonLimit = now + 10 * 86400_000;
  const soonLots = lots.filter((lot) => {
    const at = lot.expires_at ? new Date(lot.expires_at).getTime() : null;
    return at !== null && at > now && at <= soonLimit;
  });
  const soonTotal = soonLots.reduce((sum, lot) => sum + lot.amount, 0);
  const quota = Object.entries(d?.quota ?? account.quota).sort((a, b) => b[1] - a[1]);
  // 只有额度型平台（ZCode）的「更新时间」说的是配额；其余平台别拿批次缓存时刻冒充
  const quotaAt = account.platform === 'zcode' ? (d?.quota_at ?? account.quota_at) : null;
  const lastError = error ?? account.error;
  const credDays = daysUntil(account.expires_at);
  // 额度型账号没有积分余额：主指标直接给最紧窗口的剩余百分比，别只显示「—」
  const tightest = quota.length > 0 ? Math.max(...quota.map(([, used]) => used)) : null;
  const quotaLeft = tightest === null ? null : Math.max(0, 100 - tightest);

  // 这个账号实际所属的平台（WorkBuddy 国际版走 workbuddy_intl），开关按它取
  const platformKey = featurePlatform(account.platform, account.iid);
  const checkinOn = features.on(platformKey, FEATURE_CHECKIN);
  const restoreOn = features.on(platformKey, FEATURE_RESTORE);
  const travelOn = features.on(platformKey, FEATURE_TRAVEL);
  const showTravel = travelOn && account.has_secret;

  const claims = d?.claims ?? [];
  const unit = creditUnit(account.platform);
  const tabs: { key: DetailTab; label: string }[] = [
    { key: 'overview', label: '概览' },
    { key: 'lots', label: `${unit}批次（${lots.length}）` },
    { key: 'claims', label: `领取记录（${claims.length}）` },
    ...(showTravel ? [{ key: 'travel' as const, label: '猫猫旅行' }] : []),
  ];

  return (
    <>
      <Dialog
        open
        xwide
        title={account.name}
        subtitle={
          <span className="flex flex-wrap items-center gap-1.5">
            <Tag>{account.platform_label}</Tag>
            {isCurrent && <Tag tone="ok">当前登录</Tag>}
            {account.requires_checkin && checkinOn && (
              <Tag tone={checked ? 'ok' : 'warn'}>{checked ? '今日已签' : '今日未签'}</Tag>
            )}
            {!account.has_secret && <Tag tone="warn">无凭据</Tag>}
          </span>
        }
        onClose={onClose}
        actions={
          <>
            {/* 动作固定在弹窗顶部：签到（每天）→ 领取 → 刷新 → 授权 → 改名 / 备注 */}
            {account.requires_checkin && checkinOn && (
              <Tooltip content={checked ? '今天已经签到过了，明天再来' : '签到：立刻为这个账号执行今日签到'}>
                <Button
                  size="sm"
                  variant="primary"
                  onClick={runOne}
                  loading={busy === 'checkin'}
                  disabled={!!busy || !account.has_secret || checked}
                >
                  <IconCalendar size={13} />
                  签到
                </Button>
              </Tooltip>
            )}
            {account.platform === 'zcode' && (
              <Tooltip content="领取活动：拉取当前可领的活动套餐；领取时会打开浏览器过验证码，也可以在设置里开自动领取">
                <Button
                  size="sm"
                  onClick={openClaim}
                  loading={busy === 'plans'}
                  disabled={!!busy}
                >
                  <IconGift size={13} />
                  领取
                </Button>
              </Tooltip>
            )}
            <Tooltip content="刷新账号：重扫客户端并写回最新凭据与有效期，同时更新当前登录标记（不查积分）；刷新后仍过期才需要重新授权">
              <Button
                size="sm"
                onClick={refreshCredential}
                loading={busy === 'refresh-credential'}
                disabled={!!busy || !account.has_secret}
              >
                <IconRefresh size={13} />
                刷新
              </Button>
            </Tooltip>
            {account.platform !== 'trae' && (
              <Tooltip
                content={
                  account.has_secret
                    ? '重新授权：重新走一次平台登录，成功后覆盖已保存的凭据与到期时间'
                    : '重新授权：这个账号没有可用凭据，切不过去；成功后自动保存'
                }
              >
                <Button
                  size="sm"
                  onClick={() => setReauthOpen(true)}
                  disabled={!!busy}
                >
                  <IconLink size={13} />
                  授权
                </Button>
              </Tooltip>
            )}
            <span className="mx-0.5 h-4 w-px self-center bg-line" aria-hidden />
            <Tooltip content="改名：设置本机显示名，重扫客户端不会覆盖">
              <Button
                size="sm"
                variant="ghost"
                onClick={openAlias}
                disabled={!!busy}
              >
                <IconEdit size={13} />
                改名
              </Button>
            </Tooltip>
            <Tooltip content="备注：只存本机，用来记「为什么停用」「绑定在哪台机器」">
              <Button
                size="sm"
                variant="ghost"
                onClick={openNote}
                disabled={!!busy}
              >
                <IconNotebook size={13} />
                备注
              </Button>
            </Tooltip>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          <DetailTabs tab={tab} items={tabs} onChange={setTab} />

          {tab === 'overview' && (
            <div className="flex flex-col gap-4">
              {/* 结论层：最快的到期放第一格（≤7 天变警示色），余额第二，10 天内到期合计第三；
                  建议与口径进 tooltip */}
              <div className="grid grid-cols-1 gap-px overflow-hidden rounded-lg border border-line bg-line sm:grid-cols-3">
                <Stat
                  label="最近到期"
                  value={
                    nearest?.expires_at
                      ? `${fmtDay(nearest.expires_at)} · ${
                          nearestDays !== null && nearestDays >= 0
                            ? `剩 ${nearestDays} 天`
                            : fmtAmount(nearest.amount)
                        }`
                      : isDoubao
                        ? '平台未提供'
                        : '—'
                  }
                  hint={
                    nearest
                      ? nearestDays !== null && nearestDays <= 7
                        ? `只剩 ${Math.max(0, nearestDays)} 天，优先用掉`
                        : '积分批次里最近的一笔'
                      : isDoubao
                        ? '豆包接口只回累计额度，不下发批次到期日。官方规则：赠送额度自发放起 30 天内有效，这里不推算'
                        : '平台没有返回批次到期时间'
                  }
                  tone={nearestDays !== null && nearestDays <= 7 ? 'warn' : undefined}
                  strong
                />
                <Stat
                  label={
                    quota.length > 0
                      ? '积分 / 配额'
                      : isDoubao
                        ? '累计体验额度'
                        : `${unit}余额`
                  }
                  value={
                    quotaLeft !== null && (d?.balance ?? account.balance) === null
                      ? `剩 ${Math.round(quotaLeft)}%`
                      : fmtAmount(d?.balance ?? account.balance)
                  }
                  hint={
                    quota.length > 0
                      ? '额度型账号，明细见下方窗口'
                      : isDoubao
                        ? '豆包平台口径的累计已发放额度（含已消耗），不是剩余可用'
                        : '平台实时返回'
                  }
                  tone={leftTone(quotaLeft) ?? undefined}
                />
                <Stat
                  label="10 天内到期"
                  value={lots.length === 0 ? '—' : soonTotal > 0 ? fmtAmount(soonTotal) : '无'}
                  hint={
                    lots.length === 0
                      ? '还没有批次明细：签到成功或刷新积分后才有数据'
                      : soonTotal > 0
                        ? `未来 10 天有 ${soonLots.length} 笔到期，合计 ${fmtAmount(soonTotal)}；已过期的批次不计入`
                        : '10 天内没有要过期的积分'
                  }
                  tone={soonTotal > 0 ? 'warn' : undefined}
                />
              </div>

              {/* 账号信息：这是谁、凭据能不能用、会话能不能恢复 */}
              <section className="overflow-hidden rounded-lg border border-line bg-card shadow-card">
                <div className="border-b border-line px-4 py-2.5">
                  <CardTitle>账号信息</CardTitle>
                </div>
                <dl className="grid grid-cols-[max-content_minmax(0,1fr)_max-content_minmax(0,1fr)] gap-x-6 gap-y-2 px-4 py-3 text-sm">
                  <dt className="text-muted">平台</dt>
                  <dd className="truncate">{account.platform_label}</dd>
                  {/* UID 只在这里出现：列表不渲染它，点开账号才按需取一次（契约 17） */}
                  <dt className="text-muted">UID</dt>
                  <dd className="truncate">
                    <Tooltip content={uid.data ? '点击复制 UID' : undefined}>
                      <button
                        type="button"
                        className="max-w-full cursor-pointer truncate font-mono text-fg2 hover:text-accent hover:underline"
                        onClick={async () => {
                          if (!uid.data) return;
                          await navigator.clipboard.writeText(uid.data);
                          toast('UID 已复制');
                        }}
                      >
                        {uid.data ?? (uid.isLoading ? '读取中…' : '—')}
                      </button>
                    </Tooltip>
                  </dd>
                  <dt className="text-muted">凭据</dt>
                  <dd className="truncate">
                    <Hint content={account.source || undefined} focusable={false}>
                      {account.has_secret
                        ? `已保存${account.source ? `（${account.source}）` : ''}`
                        : '未保存'}
                    </Hint>
                  </dd>
                  <dt className="text-muted">凭据到期</dt>
                  <dd
                    className={cn(
                      'truncate',
                      credDays !== null && credDays < 0 && 'text-bad',
                      credDays !== null && credDays >= 0 && credDays <= 3 && 'text-warn',
                    )}
                  >
                    <Hint
                      content="这个时间来自客户端票据（票据里没有时读 token 自带的 exp）。本应用不自己续期：客户端续过才会变，否则刷新多少次都是同一个值"
                      focusable={false}
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
                    </Hint>
                  </dd>
                  {restoreOn && (
                    <>
                      <dt className="text-muted">恢复对话</dt>
                      <dd className="truncate">{isCurrent ? '可用' : '切换后可用'}</dd>
                    </>
                  )}
                  {quotaAt && (
                    <>
                      <dt className="text-muted">配额更新</dt>
                      <dd>{fmtTime(quotaAt)}</dd>
                    </>
                  )}
                  <dt className="text-muted">最近查询</dt>
                  <dd className={cn('truncate', lastError && 'text-bad')}>
                    {lastError ? '失败' : '正常'}
                  </dd>
                </dl>
                {account.note && (
                  <p className="border-t border-line px-4 py-2 text-xs whitespace-pre-wrap text-muted">
                    <span className="text-fg2">备注：</span>
                    {account.note}
                  </p>
                )}
              </section>

              {/* 额度窗口：只有额度型平台有 */}
              {quota.length > 0 && (
                <section className="overflow-hidden rounded-lg border border-line bg-card shadow-card">
                  <div className="border-b border-line px-4 py-2.5">
                    <CardTitle right={quotaAt ? `更新于 ${fmtTime(quotaAt)}` : undefined}>
                      额度窗口
                    </CardTitle>
                  </div>
                  <div className="grid gap-x-8 gap-y-3 px-4 py-3 md:grid-cols-2">
                    {quota.map(([name, used]) => {
                      const left = Math.max(0, 100 - used);
                      const tone = leftTone(left);
                      return (
                        <div key={name} className="min-w-0">
                          <div className="mb-1 flex items-baseline gap-2 text-xs">
                            <Hint content={name} focusable={false} className="truncate text-fg2">
                              {name}
                            </Hint>
                            <span className="flex-1" />
                            <span
                              className={cn(
                                'tabular-nums',
                                tone === 'bad'
                                  ? 'text-bad'
                                  : tone === 'warn'
                                    ? 'text-warn'
                                    : 'text-muted',
                              )}
                            >
                              已用 {used.toFixed(0)}% · 剩 {left.toFixed(0)}%
                            </span>
                          </div>
                          <QuotaBar used={used} tone={tone} />
                        </div>
                      );
                    })}
                  </div>
                </section>
              )}
              {quota.length === 0 && account.platform === 'zcode' && (
                <p className="rounded-lg border border-line bg-card px-4 py-3 text-xs text-muted shadow-card">
                  这个账号没有额度明细（Free 套餐或接口未下发）。
                  {quotaAt ? `上次刷新 ${fmtTime(quotaAt)}。` : '点下方「刷新」拉一次。'}
                </p>
              )}

              {/* CodeBuddy 系登录状态：两个入口都关掉就整卡不渲染 */}
              {canSwitchCodebuddy && (
                <section className="overflow-hidden rounded-lg border border-line bg-card shadow-card">
                  <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-2.5">
                    <span className="text-xs font-semibold tracking-wide text-fg2">
                      CodeBuddy 系登录
                    </span>
                    <span className="flex-1" />
                    {ideSwitchOn && (
                      <Tooltip
                        content={
                          codebuddyInstalled('software')
                            ? '把 CodeBuddy IDE 的登录切成这个账号'
                            : '未安装 CodeBuddy IDE：先安装，或到设置里指定它的数据目录'
                        }
                      >
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => switchCodebuddy('software')}
                          loading={busy === 'switch-software'}
                          disabled={!!busy || !codebuddyInstalled('software')}
                        >
                          切换 IDE
                        </Button>
                      </Tooltip>
                    )}
                    {ideaSwitchOn && (
                      <Tooltip
                        content={
                          codebuddyInstalled('idea')
                            ? '把 IntelliJ IDEA 里 CodeBuddy 插件的登录切成这个账号'
                            : '未安装 CodeBuddy 插件：先安装，或到设置里指定 JetBrains 配置目录'
                        }
                      >
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => switchCodebuddy('idea')}
                          loading={busy === 'switch-idea'}
                          disabled={!!busy || !codebuddyInstalled('idea')}
                        >
                          切换插件
                        </Button>
                      </Tooltip>
                    )}
                  </div>
                  <div className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2.5 text-xs text-muted">
                    {codebuddyTargets.isLoading && <span>读取中…</span>}
                    {(codebuddyTargets.data ?? []).map((t) => (
                      <span key={t.key}>
                        {t.label}：
                        {!t.installed ? (
                          <span className="text-muted">未安装</span>
                        ) : t.current_name ? (
                          <span className="font-medium text-fg">
                            {t.current_name}
                            {t.edition === 'intl'
                              ? '（国际版）'
                              : t.edition === 'cn'
                                ? '（国内版）'
                                : ''}
                          </span>
                        ) : (
                          <span className="text-muted">未登录</span>
                        )}
                      </span>
                    ))}
                  </div>
                </section>
              )}

              {lastError && <Notice tone="bad">最近一次查询失败：{lastError}</Notice>}
            </div>
          )}

          {tab === 'lots' && (
            <section className="overflow-hidden rounded-lg border border-line bg-card shadow-card">
              <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-2.5">
                <CardTitle right={lots.length > 0 ? `共 ${lots.length} 笔` : undefined}>
                  积分批次
                </CardTitle>
                {account.platform === 'workbuddy' && (
                  <Button size="sm" variant="ghost" onClick={() => setRewardOpen(true)}>
                    平台奖励积分
                  </Button>
                )}
              </div>
              {lots.length === 0 ? (
                <p className="px-4 py-8 text-sm text-muted">
                  {isDoubao
                    ? '豆包不提供批次明细与到期日：接口只回累计额度。官方规则是「赠送额度自发放起 30 天内有效」，这里不推算具体日期。'
                    : '还没有批次明细：签到成功后这里会列出每笔的金额、来源与到期日。'}
                </p>
              ) : (
                <div className="max-h-[52vh] overflow-auto">
                  <DataTable
                    columns={lotColumns}
                    rows={lots.map((lot, index) => ({
                      lot,
                      key: `${lot.lot_key || lot.label}-${index}`,
                    }))}
                    rowKey={(row) => row.key}
                    dense
                    stickyHead
                    className="text-sm"
                  />
                </div>
              )}
            </section>
          )}

          {tab === 'claims' && (
            <section className="overflow-hidden rounded-lg border border-line bg-card shadow-card">
              <div className="border-b border-line px-4 py-2.5">
                <CardTitle right="最近 100 条 · 签到 / 活动 / 猫猫旅行">领取记录</CardTitle>
              </div>
              <div className="max-h-[56vh] overflow-auto">
                <DataTable
                  columns={claimColumns(unit)}
                  rows={claims}
                  rowKey={(row) => row.event_key}
                  loading={detail.isLoading}
                  dense
                  stickyHead
                  // 默认最新在上：点表头可切「最早在上」，或按积分/来源排
                  defaultSort={{ key: 'date', dir: 'desc' }}
                  empty={
                    <p className="py-6 text-sm text-muted">
                      暂无领取记录；余额快照、旧批次和已领取状态不会补记。
                    </p>
                  }
                />
              </div>
            </section>
          )}

          {tab === 'travel' && showTravel && (
            <section className="overflow-hidden rounded-lg border border-line bg-card shadow-card">
              <TravelCard account={account} />
            </section>
          )}
        </div>
      </Dialog>

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

      <Dialog open={rewardOpen} title="平台奖励积分" onClose={() => setRewardOpen(false)} wide>
        {reward.isLoading && <p className="text-xs text-muted">正在读取平台明细…</p>}
        {reward.isError && <p className="text-xs text-bad">读取失败：{String(reward.error)}</p>}
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
              <DataTable
                columns={rewardColumns}
                rows={reward.data.packages}
                rowKey={(pkg) => pkg.name}
                dense
                className="text-xs"
              />
            )}
          </div>
        )}
      </Dialog>

      <ReauthDialog
        target={reauthOpen ? account : null}
        onClose={() => setReauthOpen(false)}
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
                {plan.grants.length > 0 && (
                  <div className="mt-1 text-xs">{plan.grants.join('；')}</div>
                )}
              </div>
            ))}
          </div>
        )}
      </Dialog>
    </>
  );
}
