// 主题：跟随系统 / 浅 / 深 / 经典浅 / 经典深。选择持久化在 kv（键 ui.theme，与 v1 一致）。
// 全部颜色走 index.css 的令牌表，每种主题定义同一套键——换肤只换值不换结构。
import { queryCmd } from '@/lib/cmd';

export type ThemeChoice = '' | 'light' | 'dark' | 'classic-light' | 'classic-dark';

export function applyTheme(choice: ThemeChoice): void {
  const root = document.documentElement;
  if (choice) {
    root.setAttribute('data-theme', choice);
  } else {
    root.removeAttribute('data-theme');
  }
}

export async function loadTheme(): Promise<ThemeChoice> {
  try {
    const saved = await queryCmd('settings_get', { key: 'ui.theme' });
    applyTheme((saved as ThemeChoice) || '');
    return (saved as ThemeChoice) || '';
  } catch {
    return '';
  }
}

export async function saveTheme(choice: ThemeChoice): Promise<void> {
  applyTheme(choice);
  await queryCmd('settings_set', { key: 'ui.theme', value: choice || null }).catch(() => {});
}

// 主题变化订阅：canvas 图表（ECharts）拿不到 CSS 变量，切换主题后必须由 JS
// 重读令牌重绘。两条变化路径都要覆盖——设置里显式换肤改的是 data-theme；
// 「跟随系统」下操作系统切换深浅色时 attribute 不动，靠媒体查询自己变。
let listeners = new Set<() => void>();

/** 订阅主题变化（含跟随系统模式下 OS 层的深浅切换）。返回解绑函数。 */
export function onThemeChange(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

function notifyThemeChange(): void {
  for (const cb of listeners) cb();
}

// 观察两处源头；本项目是桌面应用（一个 documentElement），常驻观察没有成本。
new MutationObserver(notifyThemeChange).observe(document.documentElement, {
  attributes: true,
  attributeFilter: ['data-theme'],
});
window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', notifyThemeChange);
