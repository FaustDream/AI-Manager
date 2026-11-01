// 渲染层兜底：任何页面抛错都给「重载界面 / 退出」，不再是无声白屏。
// 白屏 = 用户只能杀进程，这是「出了问题整个软件都没法用」的根源。
import { Component, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  render() {
    if (!this.state.error) {
      return this.props.children;
    }
    return (
      <div className="flex h-screen flex-col items-center justify-center gap-3 bg-bg px-6 text-center">
        <p className="text-sm font-semibold text-fg">界面出错了</p>
        <pre className="max-w-[560px] overflow-auto whitespace-pre-wrap rounded-md border border-line bg-card p-3 text-xs text-muted">
          {this.state.error.message}
        </pre>
        <div className="flex gap-2">
          <Button variant="primary" onClick={() => location.reload()}>
            重载界面
          </Button>
          <Button onClick={() => void invoke('app_quit')}>退出</Button>
        </div>
      </div>
    );
  }
}
