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
