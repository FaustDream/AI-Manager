import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { cn } from './button';

// 模态弹窗：Esc 可关，点背景也可关。
//
// 结构固定为「头 / 身 / 尾」三段：头尾各一条 hairline，只有中间那段滚动——
// 内容长的弹窗（技能部署、回收站）滚起来标题和按钮不动，用户不会失去位置感。
//
// 焦点纪律（总纲 P1-10）：打开时焦点进面板、Tab 在面板内循环、关闭后归还给
// 触发它的那个元素。少了这三条，键盘用户会「焦点跑到弹窗背后的页面上」，
// 按 Tab 一路点到看不见的按钮，等于弹窗对键盘不可用。
const FOCUSABLE =
  'a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';

export function Dialog({
  open,
  title,
  subtitle,
  children,
  footer,
  onClose,
  wide,
}: {
  open: boolean;
  title: string;
  /** 标题下一行小字：一句话说清「这个弹窗会对你做什么」，替代正文里的长解释 */
  subtitle?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  wide?: boolean;
}) {
  const panel = useRef<HTMLDivElement | null>(null);
  // onClose 基本都是调用方现写的箭头函数。**绝不能进 effect 依赖**：
  // 那样父组件每次重渲染（例如在弹窗里打一个字）都会重跑效应、把焦点拽回第一个元素，
  // 输入框直接没法连续输入。用 ref 读最新值，效应只跟 open 走。
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    if (!open) return;
    // 记住是谁打开的：关闭后把焦点还回去，键盘用户不会掉到页面顶部
    const opener = document.activeElement as HTMLElement | null;
    const node = panel.current;
    const items = () => Array.from(node?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    // 初始聚焦给第一个可聚焦元素（通常是关闭按钮或第一个输入框）
    items()[0]?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        closeRef.current();
        return;
      }
      if (e.key !== 'Tab') return;
      const list = items();
      if (list.length === 0) return;
      const first = list[0];
      const last = list[list.length - 1];
      const active = document.activeElement;
      // 首尾循环：Shift+Tab 从第一个绕到最后一个，Tab 从最后一个绕回第一个
      if (e.shiftKey && (active === first || !node?.contains(active))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
      // 触发者可能已经被卸载（例如确认后那一行被删了），isConnected 兜一层
      if (opener?.isConnected) opener.focus();
    };
  }, [open]);

  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-[rgba(16,20,28,.42)] p-6"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={cn(
          'view-enter flex max-h-[85vh] w-full flex-col overflow-hidden rounded-xl border border-line bg-bg',
          'shadow-[0_1px_2px_rgba(16,24,40,.06),0_18px_44px_-14px_rgba(16,24,40,.34)]',
          wide ? 'max-w-[560px]' : 'max-w-[420px]',
        )}
      >
        <header className="flex items-start gap-3 border-b border-line px-5 py-3.5">
          <div className="min-w-0 flex-1">
            <h3 className="text-md leading-5 font-semibold">{title}</h3>
            {subtitle && <p className="mt-1 text-xs leading-4 text-muted">{subtitle}</p>}
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭"
            title="关闭（Esc）"
            className="-mt-0.5 -mr-1 grid h-6 w-6 shrink-0 cursor-pointer place-items-center rounded-sm text-muted transition-colors duration-100 hover:bg-hover hover:text-fg"
          >
            <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" aria-hidden>
              <path
                d="M4 4l8 8M12 4l-8 8"
                stroke="currentColor"
                strokeWidth="1.6"
                strokeLinecap="round"
              />
            </svg>
          </button>
        </header>
        <div className="min-h-0 flex-1 overflow-auto px-5 py-4">{children}</div>
        {footer && (
          <footer className="flex justify-end gap-2 border-t border-line px-5 py-3">{footer}</footer>
        )}
      </div>
    </div>
  );
}
