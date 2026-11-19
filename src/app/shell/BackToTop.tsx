// 回到顶部：只在内容滚长了才出现。
//
// 工具型页面里它是「找不着北」的兜底，不是主操作 —— 所以做成低调的圆形小按钮：
// 滚过一屏才淡入，回到顶部后立刻收回；隐藏时同时退出 Tab 序列，
// 否则键盘用户会在看不见的按钮上停一下。
//
// 滚动容器不固定：实测页面主体、表格、列表各自可能成为滚动者（谁 overflow 谁滚），
// 所以不用「传一个 ref 进来」，而是用捕获阶段统一监听所有滚动事件，
// 由 e.target 判定当前是谁在滚。
import { useEffect, useRef, useState } from 'react';
import { ArrowUp } from 'lucide-react';

/** 出现阈值：约一屏半。刚滚一点就弹出来会显得页面很「急」。 */
const SHOW_AFTER = 480;

export function BackToTop({ viewKey }: { viewKey: string }) {
  const [show, setShow] = useState(false);
  /** 最近一次滚过阈值的容器：点按钮时滚的就是它 */
  const scroller = useRef<HTMLElement | null>(null);

  useEffect(() => {
    // scroll 事件不冒泡，只能在捕获阶段挂到 document 上
    const onScroll = (e: Event) => {
      const el = e.target;
      if (!(el instanceof HTMLElement)) return;
      if (el.scrollTop > SHOW_AFTER) {
        scroller.current = el;
        setShow(true);
      } else if (el === scroller.current) {
        setShow(false);
      }
    };
    document.addEventListener('scroll', onScroll, { passive: true, capture: true });
    return () => document.removeEventListener('scroll', onScroll, { capture: true });
  }, []);

  // 切页重置：上一页的滚动容器已经没了，按钮不能留在新页面上
  useEffect(() => {
    scroller.current = null;
    setShow(false);
  }, [viewKey]);

  const toTop = () => {
    const el = scroller.current;
    if (!el) return;
    const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    el.scrollTo({ top: 0, behavior: reduce ? 'auto' : 'smooth' });
  };

  return (
    <button
      type="button"
      onClick={toTop}
      tabIndex={show ? 0 : -1}
      aria-hidden={!show}
      title="回到顶部"
      aria-label="回到顶部"
      className={`fixed right-5 bottom-5 z-20 grid h-9 w-9 cursor-pointer place-items-center rounded-full border border-line bg-card text-fg2 shadow-card transition-[opacity,transform,box-shadow] duration-[var(--dur-2)] ease-[var(--ease-out)] hover:text-fg hover:shadow-card-hover ${
        show ? 'opacity-100' : 'pointer-events-none translate-y-2 opacity-0'
      }`}
    >
      <ArrowUp size={16} />
    </button>
  );
}
