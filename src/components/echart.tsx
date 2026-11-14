// ECharts 的 React 薄封装：init 一次、option 变了增量 setOption、容器变了 resize、卸载 dispose。
//
// 色板与主题：canvas 吃不到 CSS 变量（手写 SVG 靠 var(--color-*) 自动换肤的那套在这里失效），
// 所以 useChartPalette 订阅主题变化重读令牌；调用方把色板织进 option
// （useMemo 依赖 [data, palette]），切主题 → 新 option → setOption，图表自动跟着换肤。
// 色板快照做了缓存：主题没变时 getSnapshot 返回同一个对象，不会引起多余渲染。
import { useEffect, useRef, useSyncExternalStore } from 'react';
import { echarts, type ChartOption, type EChartsType } from '@/lib/echarts';
import { onThemeChange } from '@/lib/theme';

/** 图表用的令牌子集：从 documentElement 现读，五种主题（含经典两套）都走同一张键表。 */
export interface ChartPalette {
  accent: string;
  accentStrong: string;
  ok: string;
  warn: string;
  bad: string;
  fg: string;
  fg2: string;
  muted: string;
  line: string;
  card: string;
  hover: string;
}

function readPalette(): ChartPalette {
  const style = getComputedStyle(document.documentElement);
  const token = (name: string) => style.getPropertyValue(name).trim();
  return {
    accent: token('--color-accent'),
    accentStrong: token('--color-accent-strong'),
    ok: token('--color-ok'),
    warn: token('--color-warn'),
    bad: token('--color-bad'),
    fg: token('--color-fg'),
    fg2: token('--color-fg2'),
    muted: token('--color-muted'),
    line: token('--color-line'),
    card: token('--color-card'),
    hover: token('--color-hover'),
  };
}

// 快照缓存：主题变了才失效重读，保证 getSnapshot 引用稳定（React 要求）。
let snapshot: ChartPalette | null = null;
const subscribeTheme = (cb: () => void) =>
  onThemeChange(() => {
    snapshot = null;
    cb();
  });

/** 当前主题下的图表色板。切主题（含跟随系统时 OS 切深浅）自动更新。 */
export function useChartPalette(): ChartPalette {
  return useSyncExternalStore(subscribeTheme, () => {
    if (!snapshot) snapshot = readPalette();
    return snapshot;
  });
}

// canvas 画不了 `inherit`：启动时读一次全站字体，图内文字与界面保持同一套。
const APP_FONT = getComputedStyle(document.documentElement).fontFamily;

/** 点击图元回调：环形图点扇区选中软件用。只带 name 够用，不透传整个 ECharts 事件。 */
export type ElementClick = (name: string) => void;

export function EChart({
  option,
  height,
  className,
  ariaLabel,
  onClick,
}: {
  option: ChartOption;
  /** 像素高度：图卡高度固定，宽度交给容器（ResizeObserver 自适应）。 */
  height: number;
  className?: string;
  ariaLabel?: string;
  onClick?: ElementClick;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<EChartsType | null>(null);
  const palette = useChartPalette(); // 织进 option 才有 Dependence；这里取一次仅为随主题重渲染
  const clickRef = useRef(onClick);
  clickRef.current = onClick;

  // init 只做一次：dispose 前的实例跨渲染复用，事件与内部状态（图例点选）不丢。
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const chart = echarts.init(host);
    chartRef.current = chart;
    // 点击图元（环形扇区选中软件）。handler 走 ref：回调换新不用重绑事件。
    chart.on('click', (event) => {
      if (typeof event.name === 'string') clickRef.current?.(event.name);
    });
    // 宽度跟随窗口/布局变化时重排画布；高度固定，不会抖
    const observer = new ResizeObserver(() => chart.resize());
    observer.observe(host);
    return () => {
      observer.disconnect();
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  // option 变化 → 增量更新。replaceMerge ['series']：系列数量可增减
  // （平台/模型条目变化），其余组件走 merge，图例点选等暂存态不被重置。
  useEffect(() => {
    void palette; // 主题变化产生新 option（调用方依赖 palette），此处只是随行重设
    chartRef.current?.setOption(option, { replaceMerge: ['series'] });
  }, [option, palette]);

  return (
    <div
      ref={hostRef}
      role={ariaLabel ? 'img' : undefined}
      aria-label={ariaLabel}
      className={className}
      style={{ height, fontFamily: APP_FONT }}
    />
  );
}
