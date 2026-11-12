// 积分列：一次查询失败不该把库里已有的余额整格抹成「查询失败」。
import { describe, expect, it } from 'vitest';
import type { AccountView } from '@/lib/types';
import { COLUMNS } from './columns';

const credit = COLUMNS.find((c) => c.key === 'credit')!;

const account = (over: Partial<AccountView> = {}) =>
  ({ iid: 'workbuddy:u1@intl', balance: 60, quota: {}, ...over }) as unknown as AccountView;

describe('积分 / 配额列', () => {
  it('本次查询失败时仍显示库里已有的余额，并把原因留在 title', () => {
    const cell = credit.cell(account(), {
      'workbuddy:u1@intl': 'HTTP 401（登录态可能过期）',
    });
    expect(cell.text).toContain('60');
    expect(cell.text).not.toBe('查询失败');
    expect(cell.title).toContain('HTTP 401');
  });

  it('没有失败时正常显示余额（固定两位小数，和平台侧对齐）', () => {
    expect(credit.cell(account(), {}).text).toBe('60.00');
    expect(credit.cell(account({ balance: 60.5 }), {}).text).toBe('60.50');
  });

  it('额度型账号查询失败时仍显示剩余百分比', () => {
    const cell = credit.cell(account({ balance: null, quota: { 周额度: 25 } }), {
      'workbuddy:u1@intl': '网络请求失败',
    });
    expect(cell.text).toContain('剩 75%');
    expect(cell.text).not.toBe('查询失败');
  });
});
