import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';
import { cn } from './button';
import { useModal } from './use-modal';

// 抽屉：从右边缘滑入的整侧面板。
//
// 与弹窗的分工看内容形状：**长而连续**的东西（日志上下文、逐行明细）进抽屉——
// 它吃满视口高度，滚起来不断流；**短而要决策**的东西（确认、表单）进弹窗。
// 焦点纪律、Esc、点遮罩关、退场时序全部与弹窗一致（useModal）。
export function Drawer({
  open,
  title,
  subtitle,
  children,
  footer,
  onClose,
  width = 560,
}: {
  open: boolean;
  title: string;
  subtitle?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  /** 面板宽度上限（px）。窄屏自动夹到视口宽减两侧留白。 */
  width?: number;
}) {
  const { panel, visible, closing } = useModal(open, onClose);
  if (!visible) return null;
  // 同 Dialog：避开页面动画容器的 transform 包含块
  return createPortal(
    <div
      className={cn(
        'fixed inset-0 z-40 flex justify-end bg-scrim',
        closing ? 'overlay-out' : 'overlay-in',
      )}
      onMouseDown={(event) => {
        if (closing) return;
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        style={{ maxWidth: `min(${width}px, calc(100vw - 1.5rem))` }}
        className={cn(
          closing ? 'drawer-out' : 'drawer-in',
          'flex h-full w-full flex-col overflow-hidden border-l border-line bg-bg shadow-pop',
        )}
      >
        <header className="flex shrink-0 items-start gap-3 border-b border-line px-5 py-3.5">
          <div className="min-w-0 flex-1">
            <h3 className="text-lg leading-6 font-semibold tracking-tight">{title}</h3>
            {subtitle && <p className="mt-1 text-xs leading-4 text-muted">{subtitle}</p>}
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭"
            title="关闭（Esc）"
            className="-mt-0.5 -mr-1 grid h-7 w-7 shrink-0 cursor-pointer place-items-center rounded-md text-muted transition-colors duration-[var(--dur-1)] hover:bg-hover hover:text-fg"
          >
            <X size={15} />
          </button>
        </header>
        <div className="min-h-0 flex-1 overflow-auto px-5 py-4">{children}</div>
        {footer && (
          <footer className="flex shrink-0 flex-wrap justify-end gap-2 border-t border-line px-5 py-3">
            {footer}
          </footer>
        )}
      </div>
    </div>,
    document.body,
  );
}