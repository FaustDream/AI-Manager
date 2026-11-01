// 主题：跟随系统 / 浅 / 深。选择持久化在 kv（键 ui.theme，与 v1 一致）。
import { call } from './ipc';

export type ThemeChoice = '' | 'light' | 'dark';

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
    const saved = await call<string | null>('settings_get', { key: 'ui.theme' });
    applyTheme((saved as ThemeChoice) || '');
    return (saved as ThemeChoice) || '';
  } catch {
    return '';
  }
}

export async function saveTheme(choice: ThemeChoice): Promise<void> {
  applyTheme(choice);
  await call('settings_set', { key: 'ui.theme', value: choice || null }).catch(() => {});
}
