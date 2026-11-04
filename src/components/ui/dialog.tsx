import { useEffect } from 'react';
import type { ReactNode } from 'react';
import { cn } from './button';

// 原生 <dialog> 的轻包装：模态、点背景不关（防误触），Esc 允许关。
//
// 结构固定为「头 / 身 / 尾」三段：头尾各一条 hairline，只有中间那段滚动——
// 内容长的弹窗（技能部署、回收站）滚起来标题和按钮不动，用户不会失去位置感。
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
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-[rgba(16,20,28,.42)] p-6"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
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
