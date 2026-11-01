// 窗口框架：侧栏导航 + 内容区 + 底部每日轮询状态条 + toast。
import { useEffect, useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  LayoutDashboard, Users, MessagesSquare, Blocks, Settings, RefreshCw,
} from 'lucide-react';
import { AnimatePresence, MotionConfig, motion } from 'motion/react';
import { capabilities } from '@/app/router';
import { ToastHost } from '@/components/ui/toast';
import { Button } from '@/components/ui/button';
import { call } from '@/lib/ipc';
import { loadTheme } from '@/lib/theme';
import { fmtTime } from '@/lib/format';
import type { RoundResult } from '@/lib/types';
import { Suspense } from 'react';
import { StateBlock } from '@/components/ui/form';

const ICONS: Record<string, React.ComponentType<{ size?: number }>> = {
  overview: LayoutDashboard,
  'account-center': Users,
  sessions: MessagesSquare,
  hub: Blocks,
  settings: Settings,
};

export function Shell() {
  const caps = useMemo(() => capabilities(), []);
  const [active, setActive] = useState(caps[0]?.manifest.id ?? '');

  useEffect(() => {
    void loadTheme();
    // 挂载完成通知后端：IPC 通道正常（ui_ready 的意义见 Rust 侧注释）
    call('ui_ready').catch(() => {});
  }, []);

  const daily = useQuery<RoundResult>({
    queryKey: ['daily'],
    queryFn: () => call<RoundResult>('daily_state'),
  });

  const current = caps.find((c) => c.manifest.id === active) ?? caps[0];

  return (
    <MotionConfig reducedMotion="user">
      <div className="flex h-screen flex-col">
      <header className="flex items-center gap-2 border-b border-line px-5 py-2.5">
        <h1 className="text-lg font-semibold">AI Manager</h1>
        <nav className="ml-1 flex gap-1">
          {caps.map((cap) => {
            const Icon = ICONS[cap.manifest.id] ?? LayoutDashboard;
            return (
              <button
                key={cap.manifest.id}
                className={`flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm ${
                  active === cap.manifest.id ? 'bg-hover font-semibold text-fg' : 'text-muted hover:bg-hover hover:text-fg'
                }`}
                onClick={() => setActive(cap.manifest.id)}
              >
                <Icon size={15} />
                {cap.manifest.title}
              </button>
            );
          })}
        </nav>
        <span className="flex-1" />
        <Button
          size="sm"
          variant="ghost"
          title="立即跑一轮每日轮询"
          onClick={async () => {
            // 结果由事件桥 checkin:round 统一失效 ['accounts']，这里不猜完成时刻
            await call('daily_run_now').catch(() => {});
          }}
        >
          <RefreshCw size={14} />
        </Button>
      </header>

      <main className="flex-1 overflow-auto px-5 pb-16 pt-4">
        <AnimatePresence mode="wait" initial={false}>
          {current ? (
            <motion.div
              key={current.manifest.id}
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -4, transition: { duration: 0.08 } }}
              transition={{ duration: 0.15, ease: 'easeOut' }}
            >
              <Suspense fallback={<StateBlock kind="empty" title="加载中…" />}>
                <current.page />
              </Suspense>
            </motion.div>
          ) : (
            <StateBlock kind="error" title="没有找到任何能力目录" />
          )}
        </AnimatePresence>
      </main>

      <footer className="fixed bottom-0 left-5 right-5 flex items-center gap-2.5 border-t border-line bg-bg py-1.5 text-xs text-muted">
        <span>
          每日轮询：
          {daily.data ? `${fmtTime(daily.data.at)} · ${daily.data.message}` : '启动 15 秒后自动跑第一轮'}
        </span>
      </footer>
      <ToastHost />
      </div>
    </MotionConfig>
  );
}
