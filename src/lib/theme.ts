// 视觉风格：跟随系统 / 简约商务 / 暗色科技 / 柔和现代 / 高级极简。
// 选择持久化在 kv（键 ui.theme）；全部颜色走 index.css 的令牌表，
// 每套风格定义同一套键——换肤只换值不换结构。
//
// 字号档位也在这里落到 documentElement（同一件事：把外观偏好写成根属性），
// 但**存储位置不同**：风格是 kv，字号是 localStorage（纯显示偏好，见 ui-state.ts）。
import { useSyncExternalStore } from 'react';
import { queryCmd } from '@/lib/cmd';
import { loadFontChoice, saveFontChoice, saveThemeHint, type FontChoice } from '@/lib/ui-state';

export type ThemeChoice = '' | 'business' | 'tech' | 'soft' | 'minimal';

/** 风格元数据：设置页的预览卡与顶栏快捷菜单共用这一份，不各写一套文案。 */
export interface ThemeStyle {
  key: Exclude<ThemeChoice, ''>;
  name: string;
  note: string;
  /** 预览色带：画布 / 表面 / 主色 / 副色，取自该风格的真实令牌值 */
  swatch: [string, string, string, string];
  dark: boolean;
}

export const THEME_STYLES: readonly ThemeStyle[] = [
  {
    key: 'business',
    name: '简约商务风',
    note: '浅灰画布 · 白色卡片 · 蓝色强调',
    swatch: ['#f3f5f9', '#ffffff', '#2563eb', '#0891b2'],
    dark: false,
  },
  {
    key: 'tech',
    name: '暗色科技风',
    note: '深色底 · 紫蓝点缀 · 发光元素',
    swatch: ['#0a0c12', '#171b28', '#8b6bff', '#22d3ee'],
    dark: true,
  },
  {
    key: 'soft',
    name: '柔和现代风',
    note: '低饱渐变 · 大圆角 · 轻阴影',
    swatch: ['#f1f0fb', '#ffffff', '#6366f1', '#22d3ee'],
    dark: false,
  },
  {
    key: 'minimal',
    name: '高级极简风',
    note: '暖纸留白 · 克制配色 · 精细排版',
    swatch: ['#f2eee5', '#fffdf8', '#a05a2c', '#7d8471'],
    dark: false,
  },
];

const KEYS = THEME_STYLES.map((style) => style.key);

/**
 * 旧键位迁移。2.0.48 之前存的是 light / dark / classic-light / classic-dark，
 * 四套风格重做后键位改成语义名——不迁移的话老用户一进来就掉回「跟随系统」。
 * classic-dark 原先是暖黑，位置上换成柔和现代（浅色）：这是刻意的风格替换，
 * 不是丢了偏好，所以照常迁移而不是回退默认。
 */
const LEGACY: Record<string, ThemeChoice> = {
  light: 'business',
  dark: 'tech',
  'classic-light': 'minimal',
  'classic-dark': 'soft',
};

export function normalizeTheme(raw: unknown): ThemeChoice {
  if (typeof raw !== 'string' || raw === '') return '';
  if ((KEYS as string[]).includes(raw)) return raw as ThemeChoice;
  return LEGACY[raw] ?? '';
}

export function applyTheme(choice: ThemeChoice): void {
  const root = document.documentElement;
  if (choice) root.setAttribute('data-theme', choice);
  else root.removeAttribute('data-theme');
  saveThemeHint(choice);
}

/** 字号：标准档不设属性（用 @theme 默认值），大档覆盖 --text-* 令牌。 */
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

// ── 当前风格的可观察快照 ────────────────────────────────────────────────
// 顶栏快捷菜单与设置页都要读同一份选择：各自 useState 会出现「顶栏切了、
// 设置里的下拉还停在旧值」。这里做唯一真源，两处都订阅它。
let current: ThemeChoice = '';
const choiceListeners = new Set<() => void>();

function setCurrent(next: ThemeChoice): void {
  if (current === next) return;
  current = next;
  for (const cb of choiceListeners) cb();
}

export async function loadTheme(): Promise<ThemeChoice> {
  try {
    const saved = await queryCmd('settings_get', { key: 'ui.theme' });
    const choice = normalizeTheme(saved);
    applyTheme(choice);
    setCurrent(choice);
    // 迁移过的值顺手写回，下次启动不用再猜旧键
    if (typeof saved === 'string' && saved !== '' && saved !== choice) {
      await queryCmd('settings_set', { key: 'ui.theme', value: choice }).catch(() => {});
    }
    return choice;
  } catch {
    return '';
  }
}

export async function saveTheme(choice: ThemeChoice): Promise<void> {
  applyTheme(choice);
  setCurrent(choice);
  await queryCmd('settings_set', { key: 'ui.theme', value: choice || null }).catch(() => {});
}

/** 订阅当前风格选择（顶栏菜单与设置页共用）。 */
export function useThemeChoice(): ThemeChoice {
  return useSyncExternalStore(
    (cb) => {
      choiceListeners.add(cb);
      return () => choiceListeners.delete(cb);
    },
    () => current,
  );
}

// ── 令牌变化订阅 ────────────────────────────────────────────────────────
// canvas 图表（ECharts）拿不到 CSS 变量，切换风格后必须由 JS 重读令牌重绘。
// 两条变化路径都要覆盖——显式换肤改的是 data-theme；「跟随系统」下操作系统
// 切换深浅色时 attribute 不动，靠媒体查询自己变。字号档位同理（图内文字要跟）。
let listeners = new Set<() => void>();

/** 订阅令牌变化（含跟随系统模式下 OS 层的深浅切换、字号档位切换）。返回解绑函数。 */
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
  attributeFilter: ['data-theme', 'data-font'],
});
window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', notifyThemeChange);