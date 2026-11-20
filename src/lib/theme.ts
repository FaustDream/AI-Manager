// 主题：跟随系统 / 浅 / 深 / 经典浅 / 经典深。选择持久化在 kv（键 ui.theme）。
// 全部颜色走 index.css 的令牌表，每种主题定义同一套键——换肤只换值不换结构。
//
// 字号档位也在这里落到 documentElement（同一件事：把外观偏好写成根属性），
// 但**存储位置不同**：主题是 kv，字号是 localStorage（纯显示偏好，见 ui-state.ts）。
import { queryCmd } from '@/lib/cmd';
import { loadFontChoice, saveFontChoice, type FontChoice } from '@/lib/ui-state';

export type ThemeChoice = '' | 'light' | 'dark' | 'classic-light' | 'classic-dark';

export function applyTheme(choice: ThemeChoice): void {
  const root = document.documentElement;
  if (choice) {
    root.setAttribute('data-theme', choice);
  } else {
    root.removeAttribute('data-theme');
  }
}

/** 字号：标准档不设属性（用 @theme 默认值），大档覆盖 --text-* 四个令牌。 */
export function applyFont(choice: FontChoice): void {
  const root = document.documentElement;
  if (choice === 'large') root.setAttribute('data-font', 'large');
  else root.removeAttribute('data-font');
}

/** 启动时读一次并落地：字号不落后端，首屏即生效，不会先小后大闪一下。 */
export function loadFont(): FontChoice {
  const choice = loadFontChoice();
  applyFont(choice);
  return choice;
}

export function saveFont(choice: FontChoice): void {
  applyFont(choice);
  saveFontChoice(choice);
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
