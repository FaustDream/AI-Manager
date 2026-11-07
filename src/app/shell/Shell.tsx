// 窗口框架：顶部导航 + 内容区 + toast。
import { useEffect, useMemo, useState } from 'react';
import {
  Users, MessagesSquare, Blocks, Settings, RefreshCw, ScrollText, LayoutDashboard, BarChart3,
} from 'lucide-react';
import { AnimatePresence, MotionConfig, motion } from 'motion/react';
import { capabilities } from '@/app/router';
import { ToastHost, toast } from '@/components/ui/toast';
import { ConfirmHost } from '@/components/ui/confirm';
import { Button } from '@/components/ui/button';
import { queryCmd, runCmd } from '@/lib/cmd';
import { CommandPalette, type PaletteItem } from './CommandPalette';
import { hideSplash } from '@/lib/splash';
import { loadTheme } from '@/lib/theme';
import { Suspense } from 'react';
import { StateBlock } from '@/components/ui/form';

const ICONS: Record<string, React.ComponentType<{ size?: number }>> = {
  overview: LayoutDashboard,
  'account-center': Users,
  sessions: MessagesSquare,
  hub: Blocks,
  usage: BarChart3,
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

  // 能力页之间的轻量跳转：总览等页面只发事件，不必知道导航是怎么实现的。
  // 只在能力清单里存在的 id 生效，避免页面写错 id 时整页空白。
  useEffect(() => {
    const onNavigate = (event: Event) => {
      const id = (event as CustomEvent<string>).detail;
      if (caps.some((cap) => cap.manifest.id === id)) setActive(id);
    };
    window.addEventListener('aim:navigate', onNavigate);
    return () => window.removeEventListener('aim:navigate', onNavigate);
  }, [caps]);

  const current = caps.find((c) => c.manifest.id === active) ?? caps[0];

  const paletteItems = useMemo<PaletteItem[]>(() => {
    const pages: PaletteItem[] = caps.map((cap) => ({
      id: `nav:${cap.manifest.id}`,
      title: cap.manifest.title,
      group: '页面',
      run: () => setActive(cap.manifest.id),
    }));
    const actions: PaletteItem[] = [
      {
        id: 'act:round',
        title: '跑一轮签到轮询',
        group: '动作',
        hint: '会调用平台接口',
        run: async () => {
          if ((await runCmd('daily_run_now')) !== undefined) toast('已开始跑一轮每日轮询');
        },
      },
      {
        id: 'act:diagnose',
        title: '批量诊断全部账号',
        group: '动作',
        hint: '只读',
        run: async () => {
          const rows = await queryCmd('account_list');
          const iids = rows.filter((row) => row.has_secret && row.enabled).map((row) => row.iid);
          if (iids.length === 0) {
            toast('没有可诊断的账号');
            return;
          }
          const result = await runCmd('account_batch_diagnose', { iids });
          if (result === undefined) return;
          toast(`批量诊断：${result.total} 个 · 成功 ${result.ok} · 失败 ${result.failed.length}`);
        },
      },
      {
        id: 'act:bundle',
        title: '导出诊断包',
        group: '动作',
        hint: '日志 + 脱敏状态',
        run: async () => {
          const path = await runCmd('diagnose_bundle');
          if (path !== undefined) toast(`诊断包已导出：${path}`);
        },
      },
      {
        id: 'act:update',
        title: '检查更新',
        group: '动作',
        run: async () => {
          const info = await runCmd('update_check');
          if (info === undefined) return;
          toast(info.has_update ? `有新版本 v${info.latest}` : `已是最新 v${info.current}`);
        },
      },
      {
        id: 'act:data',
        title: '打开数据目录',
        group: '动作',
        run: () => void runCmd('maintenance_open_data'),
      },
    ];
    return [...pages, ...actions];
  }, [caps]);

  return (
    <MotionConfig reducedMotion="user">
      <div className="flex h-screen flex-col">
      <header className="flex items-center gap-2 border-b border-line px-5 py-2.5">
        {/* 窗口收到最小宽度时导航整体横向收拢滚动，按钮自身不压缩、文字不折行 */}
        <nav className="flex min-w-0 gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {caps.map((cap) => {
            const Icon = ICONS[cap.manifest.id] ?? Users;
            return (
              <button
                key={cap.manifest.id}
                className={`flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1 text-sm whitespace-nowrap ${
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

      <CommandPalette items={paletteItems} />

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
