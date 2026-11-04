import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { AppProviders } from '@/app/providers';
import { ErrorBoundary } from '@/app/shell/ErrorBoundary';
import { ReloadIndicator } from '@/app/shell/ReloadIndicator';
import { Shell } from '@/app/shell/Shell';
import { call } from '@/lib/ipc';
import { armSplashTimeout } from '@/lib/splash';
import '@/index.css';

// 窗口在 tauri.conf 里是 visible:false：WebView2 初始化那几百毫秒是一块白底，
// 直接显示就是「白屏启动」。这个模块能跑到，说明 HTML 已经解析完、启动画面已经上屏，
// 所以在这里把窗口显示出来——用户第一眼看到的是启动画面，不是白屏。
// 页面渲染完成后由 Shell 里的 PageReady 撤掉启动画面。
void call('app_show_window').catch(() => {});
// 兜底：页面 chunk 加载失败时也不会永远停在启动画面上
armSplashTimeout();

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
