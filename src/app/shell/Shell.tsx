// 应用外壳：侧边栏 / 顶部标签 / 图标轨道三种导航布局 + 内容区 + 全局浮层。
//
// 切标签「不许白屏」的四条约定（每一条都对应一次实测踩坑）：
//   1. 页面 chunk 空闲预热 —— 点导航时模块多半已在内存里；
//   2. 切页前先 `await cap.load()`：**chunk 到位了才换页面**，没到位就继续显示
//      旧页面（顶栏只多一条细进度线）。这样根本用不到「加载中骨架」这一幕；
//   3. 真正的切换放进 `startTransition`：React 可以被打断，也不会因为一帧没准备好
//      就把整块内容退回 fallback；
//   4. 只做入场动画（0.16s 淡入 + 上移），不做退出动画：带退场的
//      `<AnimatePresence mode="wait">` 会在退场结束后用**自己的 state** 去挂新页面，
//      那次渲染不在 transition 里，lazy 页面一挂起就退回灰条骨架——用户看到的就是
//      「切过去一片空白」。
import { useQuery } from '@tanstack/react-query';
import { useCallback, useEffect, useMemo, useRef, useState, useTransition } from 'react';
import { MotionConfig, motion } from 'motion/react';
import { capabilities } from '@/app/router';
import { ToastHost, toast } from '@/components/ui/toast';
import { ConfirmHost } from '@/components/ui/confirm';
import { BackToTop } from './BackToTop';
import { cn } from '@/components/ui/button';
import { queryCmd, runCmd } from '@/lib/cmd';
import { onIdle } from '@/lib/idle';
import { CommandPalette, type PaletteItem } from './CommandPalette';
import { Navbar } from './Navbar';
import { Sidebar } from './Sidebar';
import { navIcon } from './nav-icons';
import { useDesktopNotifications } from '@/lib/notify';
import { focusSession } from '@/lib/nav';
import type { Filter, Page, SessionRow } from '@/lib/types';
import { hideSplash } from '@/lib/splash';
import { loadFont, loadTheme } from '@/lib/theme';
import {
  NAV_LAYOUTS,
  NAV_KEY,
  usePersistedChoice,
  usePersistedFlag,
  type NavLayout,
} from '@/lib/ui-state';
import { Suspense } from 'react';
import { StateBlock } from '@/components/ui/form';

/**
 * 入场动画。**不从 opacity 0 起步**：0 起步意味着那一帧什么都没有，
 * 快速连点时看起来像闪一下白底。0.55 → 1 已经够表达「新内容进来了」。
 *
 * 方向：前进时从下方 6px 升上来，后退时从上方 6px 落下来。只做纵向——
 * 横向滑页在浏览器里会和「前进/后退手势」抢语义，短页面还会挤出一条横向滚动条。
 */
const PAGE_MOTION = {
  animate: { opacity: 1, y: 0 },
  transition: { duration: 0.16, ease: 'easeOut' },
} as const;
const PAGE_Y = 6;

export function Shell() {
  const caps = useMemo(() => capabilities(), []);
  // 导航只展示非 hidden 的能力；hidden 的仍可被 aim:navigate 打开
  const navCaps = useMemo(() => caps.filter((cap) => !cap.manifest.hidden), [caps]);
  const [active, setActive] = useState(navCaps[0]?.manifest.id ?? '');
  // 内容区跟随 `view`：它在 chunk 到位之后才更新，所以等待期间屏幕上一直是旧页面。
  // `active` 留在同步更新里，点哪个哪个立刻高亮——点完没反应比白屏还慌。
  const [view, setView] = useState(active);
  // chunk 还没到位：顶栏画一条进度线，其余什么都不变
  const [switching, setSwitching] = useState(false);
  const [pending, startTransition] = useTransition();
  // 连点保护：只认最后一次点击，先到的回包不许把页面切回去
  const latest = useRef(active);
  // 导航栈：跨页跳转（仪表盘点数字 → 账号中心）之后能原路返回。
  // 用 ref 存栈 + 一个布尔驱动按钮显隐：栈本身不参与渲染，没必要每次进栈都重渲染整壳。
  const history = useRef<string[]>([]);
  const [backTo, setBackTo] = useState<string | null>(null);
  // 切页方向：1 = 前进（从下方进），-1 = 后退（从上方进）
  const dir = useRef(1);

  // 导航布局与折叠位。两种侧栏形态各记一个折叠位：从轨道切回侧栏时
  // 不该继承「收起」，否则用户点了「侧边栏」却还是一条图标。
  const [layout] = usePersistedChoice<NavLayout>(NAV_KEY, 'sidebar', NAV_LAYOUTS);
  const [sidebarCollapsed, setSidebarCollapsed] = usePersistedFlag('nav.sidebar.collapsed', false);
  const [railExpanded, setRailExpanded] = usePersistedFlag('nav.rail.expanded', false);
  const sideNav = layout !== 'topbar';
  const collapsed = layout === 'rail' ? !railExpanded : sidebarCollapsed;
  const toggleNav = useCallback(() => {
    if (layout === 'rail') setRailExpanded(!railExpanded);
    else setSidebarCollapsed(!sidebarCollapsed);
  }, [layout, railExpanded, sidebarCollapsed, setRailExpanded, setSidebarCollapsed]);

  // 手动轮询的忙碌态：按钮自己转圈，不用户猜「点了没反应」
  const [running, setRunning] = useState(false);

  useEffect(() => {
    void loadTheme();
    loadFont(); // 同步落地：字号只读 localStorage，不用等后端
    // 挂载完成通知后端：IPC 通道正常（ui_ready 的意义见 Rust 侧注释）
    void runCmd('ui_ready');
  }, []);

  // 桌面通知：签到完成 / 登录结果 / 凭据临期的被动提醒
  useDesktopNotifications();

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
   * 模块没到位时旧页面原地不动，用户只会看到顶栏多了一条细线。
   */
  const go = useCallback(
    (id: string, opts?: { back?: boolean }) => {
      latest.current = id;
      setActive(id);
      if (id === view) {
        // 点回了当前页：把还在等的那次切页取消掉，别让进度线一直亮着
        setSwitching(false);
        return;
      }
      const cap = caps.find((c) => c.manifest.id === id);
      if (!cap) return;
      // 前进才入栈；返回自己未消费掉的栈，否则「返回」会在两页之间来回弹
      if (!opts?.back) {
        history.current = [...history.current.slice(-19), view];
        setBackTo(view);
      } else {
        setBackTo(history.current[history.current.length - 1] ?? null);
      }
      setSwitching(true);
      const fromIdx = navCaps.findIndex((c) => c.manifest.id === view);
      const toIdx = navCaps.findIndex((c) => c.manifest.id === id);
      dir.current = opts?.back || (fromIdx >= 0 && toIdx >= 0 && toIdx < fromIdx) ? -1 : 1;
      void cap
        .load()
        .then(() => {
          if (latest.current !== id) return; // 期间又点了别的导航项
          setSwitching(false);
          startTransition(() => setView(id));
        })
        .catch(() => {
          if (latest.current !== id) return;
          setSwitching(false);
          toast('页面加载失败，请刷新后重试');
        });
    },
    [caps, navCaps, view],
  );

  /** 返回上一页：只弹栈，不再入栈。*/
  const back = useCallback(() => {
    const prev = history.current.pop();
    if (prev) go(prev, { back: true });
    else setBackTo(null);
  }, [go]);

  // 能力页之间的轻量跳转：仪表盘等页面只发事件，不必知道导航是怎么实现的。
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

  /*
   * 切页进度线的最短显示时长（200ms）：本地 chunk 常在 30~60ms 内到位，
   * 只按布尔显隐的话，线会闪一下就没——比不显示更像「坏了一下」。
   */
  const busy = switching || pending;
  const [lineVisible, setLineVisible] = useState(false);
  const lineShownAt = useRef(0);
  useEffect(() => {
    if (busy) {
      lineShownAt.current = Date.now();
      setLineVisible(true);
      return;
    }
    const remain = 200 - (Date.now() - lineShownAt.current);
    if (remain <= 0) {
      setLineVisible(false);
      return;
    }
    const timer = window.setTimeout(() => setLineVisible(false), remain);
    return () => window.clearTimeout(timer);
  }, [busy]);

  // 数字键 1…N 直达对应能力页。**只在没有输入焦点、且不带修饰键时生效**：
  // 否则在搜索框里打「2」会跳页。不用 Alt+← 做返回——那是浏览器自己的后退，
  // 抢过来会把整个应用退出去（浏览器形态的硬约束）。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
      const el = document.activeElement;
      const tag = el?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || (el as HTMLElement)?.isContentEditable) return;
      const at = Number(event.key);
      if (!Number.isInteger(at) || at < 1 || at > navCaps.length) return;
      event.preventDefault();
      go(navCaps[at - 1].manifest.id);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [navCaps, go]);

  const runRound = useCallback(async () => {
    if (running) return;
    setRunning(true);
    try {
      // 结果由事件桥 checkin:round 统一失效 ['accounts']，这里不猜完成时刻
      if ((await runCmd('daily_run_now')) !== undefined) toast('已开始跑一轮每日轮询');
    } finally {
      setRunning(false);
    }
  }, [running]);

  // 会话快搜（Ctrl+K）：最近的 30 条进面板，命中后跳会话页并打开预览。
  // 正文级搜索在会话页（FTS）；面板只做标题级的快速定位。
  const recentSessions = useQuery<Page<SessionRow>>({
    queryKey: ['palette-sessions'],
    queryFn: () => {
      const filter: Filter = {
        source: null, account: null, project: null, state: null, q: null, origin: null,
        unnamed: false, pinned: false, hidden: false, body: null, tags: [], sort: 'last_at',
      };
      return queryCmd('session_page', { cursor: null, limit: 30, filter });
    },
    staleTime: 60_000,
  });

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
    const sessions: PaletteItem[] = (recentSessions.data?.items ?? []).map((row) => ({
      id: `sess:${row.kind}|${row.source_key}|${row.account}|${row.session_id}`,
      title: row.title || '（无标题）',
      group: '会话',
      hint: row.source_label,
      run: () => {
        go('sessions');
        focusSession(`${row.kind}|${row.source_key}|${row.account}|${row.session_id}`);
      },
    }));
    return [...pages, ...actions, ...sessions];
  }, [navCaps, go, recentSessions.data]);

  const openPalette = useCallback(() => window.dispatchEvent(new CustomEvent('aim:palette')), []);

  // 顶部标签布局的标签条：窗口收窄时整体横向滚动，按钮自身不压缩、文字不折行
  const tabs = (
    <nav
      className="flex min-w-0 gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      aria-label="主导航"
    >
      {navCaps.map((cap, at) => {
        const Icon = navIcon(cap.manifest.id);
        const on = active === cap.manifest.id;
        return (
          <button
            key={cap.manifest.id}
            type="button"
            className={cn(
              'flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md px-2.5 py-1.5 text-sm whitespace-nowrap',
              'transition-colors duration-[var(--dur-1)]',
              on ? 'bg-accent-soft font-medium text-fg' : 'text-fg2 hover:bg-hover hover:text-fg',
            )}
            title={`${cap.manifest.note ?? cap.manifest.title}（快捷键 ${at + 1}）`}
            aria-current={on ? 'page' : undefined}
            onClick={() => go(cap.manifest.id)}
          >
            <Icon size={15} className={on ? 'text-accent' : 'text-muted'} />
            {cap.manifest.title}
          </button>
        );
      })}
    </nav>
  );

  const fluid = current?.manifest.fluid ?? false;

  const content = (
    <main className="min-h-0 flex-1 overflow-auto">
      {/* 叙事页限宽 1440px 居中并放宽留白；表格页吃满宽度、保持紧凑密度。
          分流开关是 manifest.fluid —— 页面自己声明，壳层不猜。 */}
      <div
        className={cn(
          'w-full',
          fluid ? 'h-full px-4 py-3' : 'mx-auto max-w-[1440px] px-6 pt-5 pb-8',
        )}
      >
        {/* 兜底边界：只有「首屏第一页」会真的用到它——之后的切页都在 go() 里等到
            chunk 到位才换，不会走到 fallback。 */}
        <Suspense fallback={<PageSkeleton />}>
          {current ? (
            <motion.div
              key={current.manifest.id}
              className={fluid ? 'h-full' : undefined}
              initial={{ opacity: 0.55, y: dir.current * PAGE_Y }}
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
      </div>
    </main>
  );

  return (
    <MotionConfig reducedMotion="user">
      <div className="flex h-screen overflow-hidden">
        {sideNav && (
          <Sidebar
            caps={navCaps}
            active={active}
            collapsed={collapsed}
            onGo={go}
            onToggle={toggleNav}
          />
        )}
        <div className="flex min-w-0 flex-1 flex-col">
          <Navbar
            layout={layout}
            activeId={current?.manifest.id ?? ''}
            activeTitle={current?.manifest.title ?? ''}
            collapsed={collapsed}
            onToggleNav={toggleNav}
            tabs={layout === 'topbar' ? tabs : undefined}
            backTitle={backTo ? caps.find((c) => c.manifest.id === backTo)?.manifest.title ?? '上一页' : null}
            onBack={back}
            onPalette={openPalette}
            onRunRound={() => void runRound()}
            running={running}
            progress={lineVisible}
          />
          {content}
        </div>

        <CommandPalette items={paletteItems} placeholder="搜索页面 / 动作 / 会话…（Esc 关闭）" />
        <BackToTop viewKey={current?.manifest.id ?? ''} />
        <ToastHost />
        <ConfirmHost />
      </div>
    </MotionConfig>
  );
}

/** 首屏页面渲染出来的信号：撤掉 index.html 里的启动画面。 */
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
        <div className="h-7 w-24 skeleton-block rounded-md bg-hover" />
        <div className="h-7 w-24 skeleton-block rounded-md bg-hover" />
        <div className="h-7 w-24 skeleton-block rounded-md bg-hover" />
      </div>
      <div className="mt-1 space-y-2">
        {Array.from({ length: 9 }).map((_, index) => (
          <div key={index} className="h-9 skeleton-block rounded-md bg-hover" />
        ))}
      </div>
    </div>
  );
}