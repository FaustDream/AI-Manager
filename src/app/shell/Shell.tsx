// 窗口框架：顶部导航 + 内容区 + toast。
//
// 切标签「不许白屏」的四条约定（每一条都对应一次实测踩坑）：
//   1. 页面 chunk 空闲预热 —— 点标签时模块多半已在内存里；
//   2. 切页前先 `await cap.load()`：**chunk 到位了才换页面**，没到位就继续显示
//      旧页面（顶栏只多一条细进度线）。这样根本用不到「加载中骨架」这一幕；
//   3. 真正的切换放进 `startTransition`：React 可以被打断，也不会因为一帧没准备好
//      就把整块内容退回 fallback；
//   4. 只做入场动画（0.16s 淡入 + 上移），不做退出动画。曾经的
//      `<AnimatePresence mode="wait">` 会在退场结束后用**自己的 state** 去挂新页面，
//      那次渲染不在 transition 里，lazy 页面一挂起就退回灰条骨架——这就是用户看到的
//      「切过去一片空白」。
import { useCallback, useEffect, useMemo, useRef, useState, useTransition } from 'react';
import {
  Users, MessagesSquare, Blocks, Settings, RefreshCw, ScrollText, LayoutDashboard,
} from 'lucide-react';
import { MotionConfig, motion } from 'motion/react';
import { capabilities } from '@/app/router';
import { ToastHost, toast } from '@/components/ui/toast';
import { ConfirmHost } from '@/components/ui/confirm';
import { Button, cn } from '@/components/ui/button';
import { queryCmd, runCmd } from '@/lib/cmd';
import { onIdle } from '@/lib/idle';
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
  logs: ScrollText,
  settings: Settings,
};

/**
 * 入场动画。**不从 opacity 0 起步**：0 起步意味着那一帧什么都没有，
 * 快速连点时看起来像闪一下白底。0.55 → 1 已经够表达「新内容进来了」。
 */
const PAGE_MOTION = {
  initial: { opacity: 0.55, y: 6 },
  animate: { opacity: 1, y: 0 },
  transition: { duration: 0.16, ease: 'easeOut' },
} as const;

export function Shell() {
  const caps = useMemo(() => capabilities(), []);
  // 顶部导航只展示非 hidden 的能力；hidden 的仍可被 aim:navigate 打开
  const navCaps = useMemo(() => caps.filter((cap) => !cap.manifest.hidden), [caps]);
  const [active, setActive] = useState(navCaps[0]?.manifest.id ?? '');
  // 内容区跟随 `view`：它在 chunk 到位之后才更新，所以等待期间屏幕上一直是旧页面。
  // `active` 留在同步更新里，点哪个标签哪个立刻高亮——点完没反应比白屏还慌。
  const [view, setView] = useState(active);
  // chunk 还没到位：顶栏画一条进度线，其余什么都不变
  const [switching, setSwitching] = useState(false);
  const [pending, startTransition] = useTransition();
  // 连点保护：只认最后一次点击，先到的回包不许把页面切回去
  const latest = useRef(active);

  useEffect(() => {
    void loadTheme();
    // 挂载完成通知后端：IPC 通道正常（ui_ready 的意义见 Rust 侧注释）
    void runCmd('ui_ready');
  }, []);

  // 首屏渲染完 → 空闲时间把其余页面 chunk 逐个拉下来。
  // 一次全开等于和首屏抢带宽（dev 下一个页面是几十个模块），所以串行、每步等空闲。
  useEffect(() => {
    let cancelled = false;
    const queue = caps.filter((cap) => cap.manifest.id !== latest.current);
    const step = () => {
      if (cancelled) return;
      const cap = queue.shift();
      if (!cap) return;
      void cap.load().catch(() => {}); // 预热失败不影响使用，真点开时会再试一次
      onIdle(step);
    };
    const timer = window.setTimeout(step, 600);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [caps]);

  /**
   * 切页统一入口：**先等到 chunk，再换页面**。
   *
   * 之前是「直接换、让 Suspense 去兜加载」——兜底就是那块灰条骨架。现在把等待放在
   * 换页之前：模块没到位时旧页面原地不动，用户只会看到顶栏多了一条细线。
   */
  const go = useCallback(
    (id: string) => {
      latest.current = id;
      setActive(id);
      if (id === view) {
        // 点回了当前页：把还在等的那次切页取消掉，别让进度线一直亮着
        setSwitching(false);
        return;
      }
      const cap = caps.find((c) => c.manifest.id === id);
      if (!cap) return;
      setSwitching(true);
      void cap
        .load()
        .then(() => {
          if (latest.current !== id) return; // 期间又点了别的标签
          setSwitching(false);
          startTransition(() => setView(id));
        })
        .catch(() => {
          if (latest.current !== id) return;
          setSwitching(false);
          toast('页面加载失败，请刷新后重试');
        });
    },
    [caps, view],
  );

  // 能力页之间的轻量跳转：总览等页面只发事件，不必知道导航是怎么实现的。
  // 只在能力清单里存在的 id 生效，避免页面写错 id 时整页空白。
  useEffect(() => {
    const onNavigate = (event: Event) => {
      const id = (event as CustomEvent<string>).detail;
      if (caps.some((cap) => cap.manifest.id === id)) go(id);
    };
    window.addEventListener('aim:navigate', onNavigate);
    return () => window.removeEventListener('aim:navigate', onNavigate);
  }, [caps, go]);

  const current = caps.find((c) => c.manifest.id === view) ?? caps[0];

  const paletteItems = useMemo<PaletteItem[]>(() => {
    const pages: PaletteItem[] = navCaps.map((cap) => ({
      id: `nav:${cap.manifest.id}`,
      title: cap.manifest.title,
      group: '页面',
      run: () => go(cap.manifest.id),
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
          toast(info.has_update ? '检测到可用更新，可在设置里查看' : '已是最新');
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
  }, [navCaps, go]);

  return (
    <MotionConfig reducedMotion="user">
      <div className="flex h-screen flex-col">
      <header className="relative flex items-center gap-2 border-b border-line px-5 py-2.5">
        {/* 窗口收到最小宽度时导航整体横向收拢滚动，按钮自身不压缩、文字不折行 */}
        <nav className="flex min-w-0 gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {navCaps.map((cap) => {
            const Icon = ICONS[cap.manifest.id] ?? Users;
            return (
              <button
                key={cap.manifest.id}
                className={`flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1 text-sm whitespace-nowrap ${
                  active === cap.manifest.id ? 'bg-accent font-medium text-on-accent' : 'text-muted hover:bg-hover hover:text-fg'
                }`}
                onClick={() => go(cap.manifest.id)}
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
        {/* 切页进度线：贴在 header 底边，只做「正在换页」的提示，不占布局、不挡内容 */}
        <span
          aria-hidden
          className={cn(
            'pointer-events-none absolute inset-x-0 bottom-0 h-0.5 bg-accent/60 transition-opacity duration-150',
            switching || pending ? 'opacity-100' : 'opacity-0',
          )}
        />
      </header>

      <CommandPalette items={paletteItems} />

      <main className="flex-1 overflow-auto px-5 pb-6 pt-4">
        {/* 兜底边界：只有「首屏第一页」会真的用到它——之后的切页都在 go() 里等到
            chunk 到位才换，不会走到 fallback。 */}
        <Suspense fallback={<PageSkeleton />}>
          {current ? (
            <motion.div
              key={current.manifest.id}
              initial={PAGE_MOTION.initial}
              animate={PAGE_MOTION.animate}
              transition={PAGE_MOTION.transition}
            >
              <current.page />
              <PageReady />
            </motion.div>
          ) : (
            <StateBlock kind="error" title="没有找到任何能力目录" />
          )}
        </Suspense>
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

/** 首屏页面 chunk 还没到时占位：画成表格骨架，别让用户看到一片空白 + 一行小字。 */
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
