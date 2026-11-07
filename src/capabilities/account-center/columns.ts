// 列定义表（§3.3 的核心）：数据 → 单元格全在这张表里，加一列 = 加一行。
// 列宽持久化键 ui.columns 也按这里的 key 存（与 v1 同键，老配置直接可用）。
//
// 每列只答一个问题，别让它们互相重复：
//   凭据到期   = 本机存没存凭据、凭据到哪天为止（剩余天数不在这里说）。
//                曾经叫「登录态」，但它只是凭据的到期信息：切换登录靠的是
//                本机保存的凭据文件，不是这个日期——名字诚实一点少一半误解。
//   积分 / 配额 = 余额；额度型平台（ZCode）显示最紧窗口的剩余百分比
//   签到       = 今天签没签
// 「到期」列已删除：积分批次的到期日属于「点开这一行才关心」的细节，
// 放进详情（最近到期 + 批次表）比在列表里挤一列更清楚，列表也少一列宽度。
// 「状态」列已并入登录态与积分：它原来只区分「正常 / 查询失败」，
// 而 error 从来没被填充过，等于永远显示「正常」——白占一列。
import type { AccountView } from '@/lib/types';
import { daysUntil, fmtAmount, fmtFullDay, fmtTime } from '@/lib/format';

export type Cell = {
  text: string;
  /** 第二行小字：日期、原因这类补充信息，主行放结论 */
  sub?: string;
  tone?: 'ok' | 'warn' | 'bad' | 'muted';
  title?: string;
};

export interface ColumnDefEx {
  key: string;
  header: string;
  width: number;
  min: number;
  /** errors：本次刷新拿到的查询失败原因（运行时状态，不落库） */
  cell: (a: AccountView, errors: Record<string, string>) => Cell;
}

/** 凭据剩余天数：今天到期算 0，已过期是负数。只用来上色，不显示。 */
const leftDays = (a: AccountView) => daysUntil(a.expires_at);

export const FAIL_LABEL: Record<string, string> = {
  auth_expired: '登录失效',
  rate_limited: '平台限流',
  activity_closed: '活动未开放',
  provider: '平台错误',
  io: '网络错误',
  invalid: '配置无效',
};

/** 账号级健康标记：停用 / 最近失败，叠加在登录态单元格上。 */
const decorateHealth = (a: AccountView, cell: Cell): Cell => {
  const notes: string[] = [];
  if (!a.enabled) notes.push('已停用');
  if (a.last_fail_kind) {
    const label = FAIL_LABEL[a.last_fail_kind] ?? a.last_fail_kind;
    notes.push(`最近失败：${label}${a.last_fail_at ? `（${fmtTime(a.last_fail_at)}）` : ''}`);
  }
  if (notes.length === 0) return cell;
  return {
    ...cell,
    sub: notes.join(' · '),
    tone: !a.enabled ? 'muted' : 'bad',
    title: [
      !a.enabled ? '已停用：不参与自动签到、自动轮询与积分刷新；手动单账号操作仍可执行' : '',
      a.last_fail_kind
        ? `最近失败（${FAIL_LABEL[a.last_fail_kind] ?? a.last_fail_kind}），上次成功：${
            a.last_ok_at ? fmtTime(a.last_ok_at) : '无记录'
          }`
        : '',
      cell.title,
    ]
      .filter(Boolean)
      .join('\n'),
  };
};

/**
 * 登录态：只回答「凭据哪天到期」。
 * 剩余天数不给——同一个信息说两遍反而要人算，日期本身就是结论。
 */
const loginCell = (a: AccountView): Cell => {
  if (!a.has_secret) {
    return decorateHealth(a, {
      text: '未保存',
      tone: 'warn',
      title: '本机账号库里没有这个账号的登录凭据：不能一键切换，也不参与自动签到',
    });
  }
  if (!a.expires_at) {
    return decorateHealth(a, {
      text: '有效期未知',
      tone: 'muted',
      title: '平台没有提供登录凭据的到期时间（例如 ZCode 的令牌不含 exp），只能确认凭据已加密存在本机',
    });
  }
  const days = leftDays(a);
  const when = fmtFullDay(a.expires_at);
  if (days !== null && days < 0) {
    return decorateHealth(a, {
      text: `${when} 已过期`,
      tone: 'bad',
      title: `登录凭据已于 ${when} 过期（过期 ${Math.abs(days)} 天），需要在客户端重新登录一次，下次启动自动扫描会补存`,
    });
  }
  return decorateHealth(a, {
    text: `${when} 到期`,
    tone: days !== null && days <= 3 ? 'warn' : undefined,
    title: `登录凭据有效至 ${when}（不是积分到期）`,
  });
};

/** 积分与配额共用一列：额度型账号没有「积分」，显示最紧窗口的剩余百分比。 */
const creditCell = (a: AccountView, errors: Record<string, string>): Cell => {
  const error = errors[a.iid];
  const balance = fmtAmount(a.balance);
  const quota = Object.entries(a.quota ?? {});

  let cell: Cell;
  if (quota.length === 0) {
    cell = {
      text: balance,
      title: a.balance === null ? '还没查到余额，点「刷新积分」' : '平台实时返回的可用积分 / 额度余额',
    };
  } else {
    const [name, used] = quota.reduce((worst, item) => (item[1] > worst[1] ? item : worst));
    const left = Math.max(0, 100 - used);
    cell = {
      text: a.balance === null ? `剩 ${Math.round(left)}%` : `${balance} · 剩 ${Math.round(left)}%`,
      tone: left <= 10 ? 'bad' : left <= 30 ? 'warn' : 'muted',
      title: [
        a.balance === null ? '额度型账号，没有积分余额' : `积分余额 ${balance}`,
        ...quota.map(([key, value]) => `${key}：已用 ${value.toFixed(0)}%，剩 ${Math.max(0, 100 - value).toFixed(0)}%`),
        `最紧的是「${name}」`,
      ].join('\n'),
    };
  }

  // 一次查询失败不该把库里已有的数值抹掉：主行照旧，另用第二行与 title 说明原因。
  if (!error) return cell;
  return {
    ...cell,
    sub: '本次查询失败',
    tone: 'bad',
    title: [error, cell.title].filter(Boolean).join('\n'),
  };
};

const checkinCell = (a: AccountView): Cell => {
  if (!a.requires_checkin) return { text: '免签到', tone: 'muted' };
  if (a.checked_in === true) return { text: '已签', tone: 'ok' };
  if (a.checked_in === false) return { text: '未签', tone: 'warn' };
  if (a.next_at) return { text: `约 ${fmtTime(a.next_at)}`, tone: 'muted' };
  // 「待查询」= 平台没给出今天的签到结论（活动未开放 / 这次查询失败），不是「没签」。
  // 点「刷新积分」会顺带重查一次状态，查到已签就自动变成「已签」。
  return {
    text: '待查询',
    tone: 'muted',
    title: '平台没有返回今天的签到状态：可能活动未开放，或这次查询失败。点「刷新积分」或「签到」重试一次',
  };
};

export const COLUMNS: ColumnDefEx[] = [
  { key: 'platform', header: '平台', width: 142, min: 90, cell: (a) => ({ text: a.platform_label }) },
  {
    key: 'name',
    header: '昵称',
    width: 168,
    min: 80,
    cell: (a) => ({
      text: a.name,
      title: a.source ? `凭据来源：${a.source}` : undefined,
    }),
  },
  {
    key: 'tags',
    header: '标签',
    width: 132,
    min: 70,
    cell: (a) => ({
      text: a.tags.length > 0 ? a.tags.join(' · ') : '—',
      tone: a.tags.length > 0 ? undefined : 'muted',
      title: [a.tags.length > 0 ? `标签：${a.tags.join('、')}` : '', a.note ? `备注：${a.note}` : '']
        .filter(Boolean)
        .join('\n') || undefined,
    }),
  },
  { key: 'login', header: '凭据到期', width: 150, min: 110, cell: loginCell },
  { key: 'credit', header: '积分 / 配额', width: 128, min: 90, cell: creditCell },
  { key: 'checkin', header: '签到', width: 104, min: 70, cell: checkinCell },
];


/**
 * 「需要人处理」的排序权重：0 已坏 → 1 快到期/最近失败 → 2 正常 → 3 停用/无凭据。
 *
 * 手动拖动顺序是用户的主排序；这个权重只在用户主动切到「需处理优先」时生效，
 * 不改变 `sort_order`，也不在后台偷偷重排。
 */
export function attentionRank(a: AccountView, now = Date.now()): number {
  if (a.has_secret && a.expires_at !== null && a.expires_at < now) return 0;
  const days = daysUntil(a.expires_at);
  if (a.has_secret && days !== null && days <= 3) return 1;
  if (a.last_fail_kind) return 1;
  if (!a.has_secret || !a.enabled) return 3;
  return 2;
}
