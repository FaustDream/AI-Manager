// 桌面通知：浏览器侧的被动提醒。
//
// 口径：
// - 开关是浏览器侧偏好，存 localStorage（不进后端配置——它只对本浏览器有意义）；
// - 权限必须由用户手势触发申请（设置页的开关），页面加载时不偷偷弹权限框；
// - 浏览器权限是硬门槛：开关开着但权限没给，通知一条都发不出去。所以状态展示
//   以权限为准（`notifyPermission()`），别让开关看起来是开的；
// - 事件类提醒只在标签页退到后台时发：看着页面时 toast/列表已经在说话，
//   再弹系统通知是重复噪音；凭据临期是每天一次的要紧事，可见时也发。
import { useEffect } from 'react';
import { listen } from './events';
import { queryCmd } from './cmd';
import type { HealthReport, LoginOutcome, RoundResult } from './types';

const KEY = 'aim:notify';

// localStorage 在隐私模式或被策略禁用时，访问它乃至 getItem/setItem 都可能抛异常。
// 通知开关只是浏览器侧偏好，读写失败一律降级成「本次会话不持久化」，绝不打断主流程。
function readKey(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}
function writeKey(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    /* 隐私模式：只在本次会话生效 */
  }
}
function dropKey(key: string): void {
  try {
    window.localStorage.removeItem(key);
  } catch {
    /* 同上 */
  }
}

export type NotifyPermission = 'granted' | 'denied' | 'default' | 'unsupported';

export function notifySupported(): boolean {
  return typeof window !== 'undefined' && 'Notification' in window;
}

/** 浏览器侧的授权现状。`unsupported` = 环境里没有 Notification（含非安全上下文）。 */
export function notifyPermission(): NotifyPermission {
  if (!notifySupported()) return 'unsupported';
  return Notification.permission;
}

/** 用户开关 + 浏览器权限都到位才算启用。 */
export function notifyEnabled(): boolean {
  return (
    notifySupported() &&
    readKey(KEY) === '1' &&
    Notification.permission === 'granted'
  );
}

/**
 * 申请浏览器通知权限（必须由用户手势调用）。
 *
 * 只有 `default`（还没问过）才会弹授权框；`denied` 之后浏览器不再弹，
 * 只能让用户去站点设置里改——这一步在设置页给文案指路，别指望再点一次就出来。
 */
export async function requestNotifyPermission(): Promise<NotifyPermission> {
  if (!notifySupported()) return 'unsupported';
  if (Notification.permission !== 'default') return Notification.permission;
  const result = await Notification.requestPermission();
  return result;
}

/** 设置页开关调用：申请权限（用户手势内），成功才落开关。 */
export async function enableNotify(): Promise<boolean> {
  const permission = await requestNotifyPermission();
  if (permission !== 'granted') {
    dropKey(KEY);
    return false;
  }
  writeKey(KEY, '1');
  return true;
}

export function disableNotify(): void {
  dropKey(KEY);
}

/** 发一条系统通知。tag 相同的互相替换，不叠一堆。未启用时静默跳过。 */
export function notify(title: string, body: string, tag?: string): void {
  if (!notifyEnabled()) return;
  try {
    const n = new Notification(title, { body, tag, icon: '/favicon.png' });
    n.onclick = () => {
      window.focus();
      n.close();
    };
  } catch {
    // 个别环境（无通知守护进程等）构造即抛：提醒丢了不影响主流程
  }
}

/**
 * 设置页「试一条」：只验证浏览器这条路通不通，不要求开着开关。
 * 返回是否真的交给了浏览器（构造异常或权限不足都算失败）。
 */
export function notifyTest(): boolean {
  if (notifyPermission() !== 'granted') return false;
  try {
    const n = new Notification('AI Manager 测试通知', {
      body: '看到这条说明桌面通知已经可用',
      tag: 'aim-test',
      icon: '/favicon.png',
    });
    n.onclick = () => {
      window.focus();
      n.close();
    };
    return true;
  } catch {
    return false;
  }
}

/** 凭据临期每天最多提醒一次：localStorage 记日期，跨天自然重置。 */
function expiringAlertDue(): boolean {
  const today = new Date().toISOString().slice(0, 10);
  const key = 'aim:notify-expiring';
  if (readKey(key) === today) return false;
  writeKey(key, today);
  return true;
}

/**
 * 挂在 Shell 上的通知订阅：签到轮询 / 活动轮询 / 账号登录结果 / ZCode 领取 /
 * 领取入账，外加启动时一次凭据临期体检（数据源 = system_health 的 accounts.expiring）。
 * 未启用时订阅照常建立但 notify 静默跳过——开关随时打开随时生效，不用重挂。
 *
 * 刻意不通知的事件：`credits:round` 载荷只有账号数（拿不到失败数，
 * 全绿也弹就是纯噪音）、`log:line` 逐行推、`session:scan` 与 `accounts:scanned`
 * 是后台同步完成信号，用户没有要做的动作。
 */
export function useDesktopNotifications(): void {
  useEffect(() => {
    const unsubs: Array<() => void> = [];
    const track = (p: Promise<() => void>) => {
      void p.then((un) => unsubs.push(un));
    };

    track(
      listen<RoundResult>('checkin:round', (e) => {
        if (!document.hidden) return;
        const r = e.payload;
        notify(
          r.failed > 0 ? '签到轮询完成（有失败）' : '签到轮询完成',
          r.message,
          'checkin-round',
        );
      }),
    );
    track(
      listen<LoginOutcome>('account:login', (e) => {
        if (!document.hidden) return;
        const o = e.payload;
        notify(o.ok ? '账号登录成功' : '账号登录失败', o.message, 'account-login');
      }),
    );
    track(
      listen<string>('zcode:claim', (e) => {
        if (!document.hidden) return;
        notify('ZCode 自动领取', e.payload, 'zcode-claim');
      }),
    );
    // 活动轮询（猫猫旅行 / 每日问候）：与签到同一档，后台跑完给一条汇总。
    track(
      listen<string>('activity:round', (e) => {
        if (!document.hidden) return;
        notify('活动轮询完成', e.payload, 'activity-round');
      }),
    );
    // 领取入账：三条路径（签到 / ZCode / 旅行）写台账成功后都会推。
    // 一轮可能连推多条，统一用同一个 tag —— 后一条顶掉前一条，不刷屏；
    // 正文不带账号 iid（那是内部标识），只告诉用户去哪儿看。
    track(
      listen<string>('claim:recorded', () => {
        if (!document.hidden) return;
        notify('有新的领取入账', '去账号中心 → 点开账号详情，可看领取台账', 'claim-recorded');
      }),
    );

    // 凭据临期：启动体检一次。失败不打扰（健康报告本身会在设置页重试）。
    void queryCmd('system_health')
      .then((report: HealthReport) => {
        if (report.accounts.expiring > 0 && expiringAlertDue()) {
          notify(
            '凭据即将到期',
            `${report.accounts.expiring} 个账号的凭据 3 天内到期，去账号中心刷新或重新授权`,
            'credentials-expiring',
          );
        }
      })
      .catch(() => {});

    return () => {
      for (const un of unsubs) un();
    };
  }, []);
}
