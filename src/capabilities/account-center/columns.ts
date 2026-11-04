// 列定义表（§3.3 的核心）：数据 → 单元格全在这张表里，加一列 = 加一行。
// 列宽持久化键 ui.columns 也按这里的 key 存（与 v1 同键，老配置直接可用）。
//
// 每列只答一个问题，别让它们互相重复：
//   登录态     = 本机存没存凭据、凭据到哪天为止（剩余天数不在这里说）
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

/**
 * 登录态：只回答「凭据哪天到期」。
 * 剩余天数不给——同一个信息说两遍反而要人算，日期本身就是结论。
 */
const loginCell = (a: AccountView): Cell => {
  if (!a.has_secret) {
    return {
      text: '未保存',
      tone: 'warn',
      title: '本机账号库里没有这个账号的登录凭据：不能一键切换，也不参与自动签到',
    };
  }
  if (!a.expires_at) {
    return {
      text: '有效期未知',
      tone: 'muted',
      title: '平台没有提供登录凭据的到期时间（例如 ZCode 的令牌不含 exp），只能确认凭据已加密存在本机',
    };
  }
  const days = leftDays(a);
  const when = fmtFullDay(a.expires_at);
  if (days !== null && days < 0) {
    return {
      text: `${when} 已过期`,
      tone: 'bad',
      title: `登录凭据已于 ${when} 过期（过期 ${Math.abs(days)} 天），需要在客户端重新登录一次，再点「扫描客户端」补存`,
    };
  }
  return {
    text: `${when} 到期`,
    tone: days !== null && days <= 3 ? 'warn' : undefined,
    title: `登录凭据有效至 ${when}（不是积分到期）`,
  };
};

/** 积分与配额共用一列：额度型账号没有「积分」，显示最紧窗口的剩余百分比。 */
const creditCell = (a: AccountView, errors: Record<string, string>): Cell => {
  const error = errors[a.iid];
  if (error) return { text: '查询失败', tone: 'bad', title: error };

  const balance = fmtAmount(a.balance);
  const quota = Object.entries(a.quota ?? {});
  if (quota.length === 0) {
    return {
      text: balance,
      title: a.balance === null ? '还没查到余额，点「刷新积分」' : '平台实时返回的可用积分 / 额度余额',
    };
  }

  const [name, used] = quota.reduce((worst, item) => (item[1] > worst[1] ? item : worst));
  const left = Math.max(0, 100 - used);
  return {
    text: a.balance === null ? `剩 ${Math.round(left)}%` : `${balance} · 剩 ${Math.round(left)}%`,
    tone: left <= 10 ? 'bad' : left <= 30 ? 'warn' : 'muted',
    title: [
      a.balance === null ? '额度型账号，没有积分余额' : `积分余额 ${balance}`,
      ...quota.map(([key, value]) => `${key}：已用 ${value.toFixed(0)}%，剩 ${Math.max(0, 100 - value).toFixed(0)}%`),
      `最紧的是「${name}」`,
    ].join('\n'),
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
    cell: (a) => ({ text: a.name, title: a.source ? `凭据来源：${a.source}` : undefined }),
  },
  { key: 'login', header: '登录态', width: 150, min: 110, cell: loginCell },
  { key: 'credit', header: '积分 / 配额', width: 128, min: 90, cell: creditCell },
  { key: 'checkin', header: '签到', width: 104, min: 70, cell: checkinCell },
];

