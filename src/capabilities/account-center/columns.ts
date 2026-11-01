// 列定义表（§3.3 的核心）：数据 → 单元格全在这张表里，加一列 = 加一行。
// 列宽持久化键 ui.columns 也按这里的 key 存（与 v1 同键，老配置直接可用）。
import type { AccountView } from '@/lib/types';
import { fmtAmount, fmtTime } from '@/lib/format';

export type Cell = { text: string; tone?: 'ok' | 'warn' | 'bad' | 'muted'; title?: string };

export interface ColumnDefEx {
  key: string;
  header: string;
  width: number;
  min: number;
  cell: (a: AccountView) => Cell;
}

const checkinCell = (a: AccountView): Cell => {
  if (!a.requires_checkin) return { text: '免签到', tone: 'muted' };
  if (a.checked_in === true) return { text: '已签', tone: 'ok' };
  if (a.checked_in === false) return { text: '未签', tone: 'warn' };
  if (a.next_at) return { text: `约 ${fmtTime(a.next_at)}`, tone: 'muted' };
  return { text: '待查询', tone: 'muted' };
};

export const COLUMNS: ColumnDefEx[] = [
  { key: 'platform', header: '平台', width: 110, min: 70, cell: (a) => ({ text: a.platform_label }) },
  { key: 'name', header: '昵称', width: 150, min: 80, cell: (a) => ({ text: a.name, title: a.source }) },
  {
    key: 'state',
    header: '状态',
    width: 90,
    min: 60,
    // 只报查询异常；正常时的签到状态由「签到」列表达，两列重复白占一个列位
    cell: (a) =>
      a.error ? { text: '查询失败', tone: 'bad', title: a.error } : { text: '正常', tone: 'muted' },
  },
  {
    key: 'login',
    header: '登录态',
    width: 90,
    min: 60,
    cell: (a) =>
      a.has_secret ? { text: '已保存', tone: 'muted' } : { text: '无凭据', tone: 'warn' },
  },
  {
    key: 'credit',
    header: '积分',
    width: 100,
    min: 60,
    cell: (a) => ({ text: fmtAmount(a.balance) }),
  },
  {
    key: 'quota',
    header: '配额',
    width: 130,
    min: 80,
    cell: (a) => {
      const entries = Object.entries(a.quota ?? {});
      if (entries.length === 0) return { text: '—' };
      const [name, used] = entries[0];
      return {
        text: `${name} ${Math.round(used)}%`,
        tone: used >= 90 ? 'bad' : used >= 70 ? 'warn' : 'muted',
        title: entries.map(([k, v]) => `${k}: ${v.toFixed(1)}%`).join('\n'),
      };
    },
  },
  {
    key: 'checkin',
    header: '签到',
    width: 120,
    min: 70,
    cell: checkinCell,
  },
  {
    key: 'expires',
    header: '到期',
    width: 110,
    min: 70,
    cell: (a) => ({ text: fmtTime(a.expires_at) }),
  },
];
