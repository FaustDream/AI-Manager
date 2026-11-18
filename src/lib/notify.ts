// 桌面通知（浏览器化后接替托盘的被动提醒职责）。
//
// 口径：
// - 开关是浏览器侧偏好，存 localStorage（不进后端配置——它只对本浏览器有意义）；
// - 权限必须由用户手势触发申请（设置页的开关），页面加载时不偷偷弹权限框；
// - 事件类提醒只在标签页退到后台时发：看着页面时 toast/列表已经在说话，
//   再弹系统通知是重复噪音；凭据临期是每天一次的要紧事，可见时也发。
import { useEffect } from 'react';
import { listen } from './events';
import { queryCmd } from './cmd';
import type { HealthReport, LoginOutcome, RoundResult } from './types';

const KEY = 'aim:notify';

export function notifySupported(): boolean {
  return typeof window !== 'undefined' && 'Notification' in window;
}

/** 用户开关 + 浏览器权限都到位才算启用。 */
export function notifyEnabled(): boolean {
  return (
    notifySupported() &&
    localStorage.getItem(KEY) === '1' &&
    Notification.permission === 'granted'
  );
}

/** 设置页开关调用：申请权限（用户手势内），成功才落开关。 */
export async function enableNotify(): Promise<boolean> {
  if (!notifySupported()) return false;
  let permission = Notification.permission;
  if (permission === 'default') {
    permission = await Notification.requestPermission();
  }
  if (permission !== 'granted') {
    localStorage.removeItem(KEY);
    return false;
  }
  localStorage.setItem(KEY, '1');
  return true;
}

export function disableNotify(): void {
  localStorage.removeItem(KEY);
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

/** 凭据临期每天最多提醒一次：localStorage 记日期，跨天自然重置。 */
function expiringAlertDue(): boolean {
  const today = new Date().toISOString().slice(0, 10);
  const key = 'aim:notify-expiring';
  if (localStorage.getItem(key) === today) return false;
  localStorage.setItem(key, today);
  return true;
}

/**
 * 挂在 Shell 上的通知订阅：签到轮询完成 / 账号登录结果 / ZCode 领取，
 * 外加启动时一次凭据临期体检（数据源 = system_health 的 accounts.expiring）。
 * 未启用时订阅照常建立但 notify 静默跳过——开关随时打开随时生效，不用重挂。
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
