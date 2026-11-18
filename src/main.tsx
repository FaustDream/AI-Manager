import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { AppProviders } from '@/app/providers';
import { ErrorBoundary } from '@/app/shell/ErrorBoundary';
import { ReloadIndicator } from '@/app/shell/ReloadIndicator';
import { Shell } from '@/app/shell/Shell';
import { call } from '@/lib/ipc';
import { installContextMenuGuard } from '@/lib/context-menu';
import { armSplashTimeout } from '@/lib/splash';
import '@/index.css';

// 浏览器化后 app_show_window 是后端的空操作（没有宿主窗口可显示），
// 保留调用是为了首屏流程不分叉；启动画面由 Shell 里的 PageReady 撤掉。
void call('app_show_window').catch(() => {});
// 兜底：页面 chunk 加载失败时也不会永远停在启动画面上
armSplashTimeout();
// 桌面应用不需要浏览器原生右键菜单；开关在 lib/context-menu.ts
installContextMenuGuard();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ErrorBoundary>
      <AppProviders>
        <Shell />
      </AppProviders>
    </ErrorBoundary>
    <ReloadIndicator />
  </StrictMode>,
);
