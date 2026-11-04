// 窗口框架：顶部导航 + 内容区 + toast。
import { useEffect, useMemo, useState } from 'react';
import {
  Users, MessagesSquare, Blocks, Settings, RefreshCw, ListChecks, ScrollText,
} from 'lucide-react';
import { AnimatePresence, MotionConfig, motion } from 'motion/react';
import { capabilities } from '@/app/router';
import { ToastHost, toast } from '@/components/ui/toast';
import { ConfirmHost } from '@/components/ui/confirm';
import { Button } from '@/components/ui/button';
import { runCmd } from '@/lib/cmd';
import { hideSplash } from '@/lib/splash';
import { loadTheme } from '@/lib/theme';
import { Suspense } from 'react';
import { StateBlock } from '@/components/ui/form';

const ICONS: Record<string, React.ComponentType<{ size?: number }>> = {
  'account-center': Users,
  sessions: MessagesSquare,
  hub: Blocks,
  tasks: ListChecks,
  logs: ScrollText,
  settings: Settings,
};

export function Shell() {
  const caps = useMemo(() => capabilities(), []);
  const [active, setActive] = useState(caps[0]?.manifest.id ?? '');

  useEffect(() => {
    void loadTheme();
    // 挂载完成通知后端：IPC 通道正常（ui_ready 的意义见 Rust 侧注释）
    void runCmd('ui_ready');
  }, []);

  const current = caps.find((c) => c.manifest.id === active) ?? caps[0];

  return (
    <MotionConfig reducedMotion="user">
      <div className="flex h-screen flex-col">
      <header className="flex items-center gap-2 border-b border-line px-5 py-2.5">
        <nav className="flex gap-1">
          {caps.map((cap) => {
            const Icon = ICONS[cap.manifest.id] ?? Users;
            return (
              <button
                key={cap.manifest.id}
                className={`flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm ${
                  active === cap.manifest.id ? 'bg-accent font-medium text-on-accent' : 'text-muted hover:bg-hover hover:text-fg'
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
            if ((await runCmd('daily_run_now')) !== undefined) toast('已开始跑一轮每日轮询');
          }}
        >
          <RefreshCw size={14} />
        </Button>
      </header>

      <main className="flex-1 overflow-auto px-5 pb-6 pt-4">
        <AnimatePresence mode="wait" initial={false}>
          {current ? (
            <motion.div
              key={current.manifest.id}
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -4, transition: { duration: 0.08 } }}
              transition={{ duration: 0.15, ease: 'easeOut' }}
            >
              <Suspense fallback={<PageSkeleton />}>
                <current.page />
                <PageReady />
              </Suspense>
            </motion.div>
          ) : (
            <StateBlock kind="error" title="没有找到任何能力目录" />
          )}
        </AnimatePresence>
      </main>

      <ToastHost />
      <ConfirmHost />
      </div>
    </MotionConfig>
  );
}

/** 首屏页面渲染出来的信号：撤掉 index.html 里的启动画面。
 *  窗口在 main.tsx 里就已经显示出来了（那时启动画面已上屏），所以这里直接收。 */
function PageReady() {
  useEffect(() => {
    hideSplash();
  }, []);
  return null;
}

/** 页面 chunk 还没到时占位：画成表格骨架，别让用户看到一片空白 + 一行小字。 */
function PageSkeleton() {
  return (
    <div className="flex flex-col gap-2.5" aria-busy="true">
      <div className="flex items-center gap-2">
        <div className="h-7 w-24 animate-pulse rounded-md bg-hover" />
        <div className="h-7 w-24 animate-pulse rounded-md bg-hover" />
        <div className="h-7 w-24 animate-pulse rounded-md bg-hover" />
      </div>
      <div className="mt-1 space-y-2">
        {Array.from({ length: 9 }).map((_, index) => (
          <div key={index} className="h-9 animate-pulse rounded-md bg-hover" />
        ))}
      </div>
    </div>
  );
}
