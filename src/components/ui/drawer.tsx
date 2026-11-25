import * as RadixDialog from '@radix-ui/react-dialog';
import type { ReactNode } from 'react';
import { X } from 'lucide-react';
import { cn } from './button';

// 抽屉：Radix Dialog 底座，从右边缘滑入的整侧面板。
//
// 与弹窗的分工看内容形状：**长而连续**的东西（日志上下文、逐行明细）进抽屉——
// 它吃满视口高度，滚起来不断流；**短而要决策**的东西（确认、表单）进弹窗。
// 焦点陷阱、Esc、点遮罩关、滚动锁与焦点归还与弹窗同源；同样只挂入场动画
// （`data-[state=open]:`），关闭态立即卸载，避免 Radix Presence 等不到 animationend。
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
  return (
    <RadixDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 z-40 bg-scrim data-[state=open]:animate-[fade-in_var(--dur-2)_ease-out]" />
        <RadixDialog.Content
          aria-describedby={subtitle ? undefined : ''}
          style={{ maxWidth: `min(${width}px, calc(100vw - 1.5rem))` }}
          className={cn(
            'fixed inset-y-0 right-0 z-40 flex h-full w-full flex-col overflow-hidden border-l border-line bg-bg shadow-pop',
            'data-[state=open]:animate-[drawer-in_var(--dur-3)_ease-out]',
          )}
        >
          <header className="flex shrink-0 items-start gap-3 border-b border-line px-5 py-3.5">
            <div className="min-w-0 flex-1">
              <RadixDialog.Title asChild>
                <h3 className="text-lg leading-6 font-semibold tracking-tight">{title}</h3>
              </RadixDialog.Title>
              {subtitle && (
                <RadixDialog.Description asChild>
                  <p className="mt-1 text-xs leading-4 text-muted">{subtitle}</p>
                </RadixDialog.Description>
              )}
            </div>
            <RadixDialog.Close asChild>
              <button
                type="button"
                aria-label="关闭"
                title="关闭（Esc）"
                className="-mt-0.5 -mr-1 grid h-7 w-7 shrink-0 cursor-pointer place-items-center rounded-md text-muted transition-colors duration-[var(--dur-1)] hover:bg-hover hover:text-fg"
              >
                <X size={15} />
              </button>
            </RadixDialog.Close>
          </header>
          <div className="min-h-0 flex-1 overflow-auto px-5 py-4">{children}</div>
          {footer && (
            <footer className="flex shrink-0 flex-wrap justify-end gap-2 border-t border-line px-5 py-3">
              {footer}
            </footer>
          )}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
