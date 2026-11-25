// 积分列：一次查询失败不该把库里已有的余额整格抹成「查询失败」。
import { describe, expect, it } from 'vitest';
import type { AccountView } from '@/lib/types';
import { COLUMNS, webLoginUsable } from './columns';

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

describe('WorkBuddy 网页登录用途', () => {
  it('国内版保留签到与旅行，不承诺模型接口', () => {
    const text = webLoginUsable({ requires_checkin: true });
    expect(text).toContain('签到');
    expect(text).toContain('猫猫旅行');
    expect(text).not.toContain('模型接口');
  });
  it('国际版仅显示积分与云端校验', () => {
    expect(webLoginUsable({ requires_checkin: false })).toBe('积分余额与云端校验');
  });
});

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

  it('ima 的余额单位是「算力」，不沿用「积分」', () => {
    const cell = credit.cell(account({ iid: 'ima:u1', platform: 'ima' }), {});
    expect(cell.title).toContain('可用算力');
    expect(cell.title).not.toContain('积分');
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

  it('悬停给出最近一笔的到期日，精确到日', () => {
    const cell = expiring.cell(
      account({
        expiring_10d: 170,
        lot_expires_at: new Date(2026, 10, 28, 15, 30).getTime(),
      }),
      {},
    );
    expect(cell.title).toContain('最近一笔到期：2026-11-28');
    expect(cell.title).not.toContain('15:30');
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

// 登录态列：结论由后端按各平台凭据形态算好，这里只验渲染不串味。
const login = COLUMNS.find((c) => c.key === 'login')!;

const shape = (over: Record<string, unknown> = {}) => ({
  kind: 'profile_pack',
  label: '会话档案包',
  multi_account: true,
  switchable: true,
  expiry_known: true,
  note: '一个账号一份完整会话档案',
  ...over,
});

const loginState = (over: Record<string, unknown> = {}) => ({
  kind: 'current',
  label: '当前登录',
  detail: '这就是客户端现在登录的账号',
  shape: shape(),
  current: true,
  switchable: true,
  expires_at: null,
  ...over,
});

describe('登录态列（按平台隔离）', () => {
  it('直接渲染后端结论，并点明凭据形态与多账号', () => {
    const cell = login.cell(account({ enabled: true, login: loginState() } as never), {});
    expect(cell.text).toBe('当前登录');
    expect(cell.sub).toContain('会话档案包');
    expect(cell.title).toContain('多账号并存');
    expect(cell.tone).toBe('ok');
    expect(cell.icon).toBe('check');
  });

  it('豆包的粘贴 Cookie 不会被读成可切换的客户端凭据', () => {
    const cell = login.cell(
      account({
        enabled: true,
        platform: 'doubao',
        login: loginState({
          kind: 'unknown_expiry',
          label: '凭据已保存',
          current: false,
          switchable: false,
          shape: shape({
            kind: 'pasted_cookie',
            label: '粘贴 Cookie',
            switchable: false,
            expiry_known: false,
          }),
        }),
      } as never),
      {},
    );
    expect(cell.sub).toContain('粘贴 Cookie');
    expect(cell.sub).not.toContain('会话档案包');
    expect(cell.title).toContain('不下发凭据到期时间');
    expect(cell.title).not.toContain('可一键切换');
  });

  it('Trae 的设备绑定只读态不承诺切换', () => {
    const cell = login.cell(
      account({
        enabled: true,
        platform: 'trae',
        login: loginState({
          kind: 'saved',
          label: '凭据已保存',
          current: false,
          switchable: false,
          shape: shape({
            kind: 'device_bound',
            label: '设备绑定只读',
            multi_account: false,
            switchable: false,
          }),
        }),
      } as never),
      {},
    );
    expect(cell.sub).toContain('设备绑定只读');
    expect(cell.title).not.toContain('多账号并存');
    expect(cell.title).not.toContain('可一键切换');
  });

  it('已过期与未保存是两种结论，颜色也不同', () => {
    const expired = login.cell(
      account({ enabled: true, login: loginState({ kind: 'expired', label: '2026-11-01 已过期' }) } as never),
      {},
    );
    expect(expired.tone).toBe('bad');
    expect(expired.icon).toBe('warning');
    const missing = login.cell(
      account({ enabled: true, login: loginState({ kind: 'not_saved', label: '未保存凭据' }) } as never),
      {},
    );
    expect(missing.tone).toBe('warn');
    expect(missing.text).toBe('未保存凭据');
  });

  it('后端没给登录态时回落旧日期口径，不显示空白', () => {
    const cell = login.cell(account({ enabled: true, has_secret: true, expires_at: null } as never), {});
    expect(cell.text).toBe('有效期未知');
  });

  it('停用叠加层仍然生效，不被新结论盖掉', () => {
    const cell = login.cell(
      account({ enabled: false, login: loginState() } as never),
      {},
    );
    expect(cell.sub).toBe('已停用');
    expect(cell.tone).toBe('muted');
  });
});
