// 积分列：一次查询失败不该把库里已有的余额整格抹成「查询失败」。
import { describe, expect, it } from 'vitest';
import type { AccountView } from '@/lib/types';
import { COLUMNS } from './columns';

const credit = COLUMNS.find((c) => c.key === 'credit')!;
const checkin = COLUMNS.find((c) => c.key === 'checkin')!;

const account = (over: Partial<AccountView> = {}) =>
  ({
    iid: 'workbuddy:u1@intl',
    platform: 'workbuddy',
    balance: 60,
    quota: {},
    ...over,
  }) as unknown as AccountView;

describe('积分 / 配额列', () => {
  it('本次查询失败时仍显示库里已有的余额，并把原因留在 title', () => {
    const cell = credit.cell(account(), {
      'workbuddy:u1@intl': 'HTTP 401（登录态可能过期）',
    });
    expect(cell.text).toContain('60');
    expect(cell.text).not.toBe('查询失败');
    expect(cell.title).toContain('HTTP 401');
  });

  it('没有失败时正常显示余额（整数不带小数、带分保留两位）', () => {
    expect(credit.cell(account(), {}).text).toBe('60');
    expect(credit.cell(account({ balance: 60.5 }), {}).text).toBe('60.5');
  });

  it('额度型账号查询失败时仍显示剩余百分比', () => {
    const cell = credit.cell(account({ balance: null, quota: { 周额度: 25 } }), {
      'workbuddy:u1@intl': '网络请求失败',
    });
    expect(cell.text).toContain('剩 75%');
    expect(cell.text).not.toBe('查询失败');
  });
});

describe('10 天内到期列', () => {
  const expiring = COLUMNS.find((c) => c.key === 'expiring')!;

  it('从没拿到过批次时显示「—」，不能当成 0', () => {
    const cell = expiring.cell(account({ expiring_10d: null }), {});
    expect(cell.text).toBe('—');
    expect(cell.tone).toBe('muted');
  });

  it('有批次但 10 天内没有到期时显示「无」', () => {
    const cell = expiring.cell(account({ expiring_10d: 0 }), {});
    expect(cell.text).toBe('无');
    expect(cell.tone).toBe('muted');
  });

  it('有金额时按金额显示并标警示色，口径写在 title', () => {
    const cell = expiring.cell(account({ expiring_10d: 170 }), {});
    expect(cell.text).toBe('170');
    expect(cell.tone).toBe('warn');
    expect(cell.title).toContain('10 天内到期');
  });
});

describe('凭据到期列', () => {
  const login = COLUMNS.find((c) => c.key === 'login')!;
  const valid = {
    has_secret: true,
    enabled: true,
    expires_at: new Date(2027, 0, 1).getTime(),
    last_ok_at: new Date(2026, 10, 12, 9, 0).getTime(),
  };

  it('网络错误不染红凭据列：它跟凭据没关系，网恢复了自己就好', () => {
    const cell = login.cell(
      account({ ...valid, last_fail_kind: 'io', last_fail_at: new Date(2026, 10, 13, 9, 0).getTime() }),
      {},
    );
    expect(cell.tone).toBeUndefined();
    expect(cell.sub).toContain('网络错误');
    expect(cell.sub).toContain('与凭据无关');
  });

  it('平台限流同样不染红', () => {
    const cell = login.cell(account({ ...valid, last_fail_kind: 'rate_limited' }), {});
    expect(cell.tone).toBeUndefined();
  });

  it('凭据/账号类的失败才标红', () => {
    const cell = login.cell(account({ ...valid, last_fail_kind: 'auth_expired' }), {});
    expect(cell.tone).toBe('bad');
    expect(cell.sub).toContain('登录失效');
  });
});

describe('签到列', () => {
  it('签没签到只用图标表达，文字退到 title（否则和图标重复一遍）', () => {
    const signed = checkin.cell(account({ requires_checkin: true, checked_in: true }), {});
    expect(signed.text).toBe('');
    expect(signed.icon).toBe('check');
    expect(signed.title).toContain('已签到');

    const missing = checkin.cell(account({ requires_checkin: true, checked_in: false }), {});
    expect(missing.text).toBe('');
    expect(missing.icon).toBe('clock');
    expect(missing.title).toContain('没签到');
  });

  it('没有签到玩法的平台不画图标：没有的功能不该占一个叉号', () => {
    const cell = checkin.cell(account({ platform: 'zcode', requires_checkin: false, checked_in: null }), {});
    expect(cell.icon).toBeUndefined();
    expect(cell.text).toBe('');
    expect(cell.title).toContain('没有签到玩法');
  });

  it('在设置里关掉签到入口后，列表不再显示签到状态', () => {
    const off = () => false;
    const cell = checkin.cell(account({ requires_checkin: true, checked_in: true }), {}, off);
    expect(cell.icon).toBeUndefined();
    expect(cell.text).toBe('');
  });

  it('开关默认全开：国际版按 workbuddy_intl 取开关', () => {
    const seen: string[] = [];
    const spy = (platform: string) => {
      seen.push(platform);
      return true;
    };
    checkin.cell(account({ requires_checkin: true, checked_in: true }), {}, spy);
    expect(seen).toEqual(['workbuddy_intl']);
  });

  it('「约 HH:MM」不是签到状态，时间要继续显示', () => {
    const cell = checkin.cell(
      account({
        requires_checkin: true,
        checked_in: null,
        next_at: new Date(2026, 10, 12, 9, 30).getTime(),
      }),
      {},
    );
    expect(cell.text).toBe('约 11-12 09:30');
  });
});
