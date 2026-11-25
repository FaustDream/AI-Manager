import * as RadixDialog from '@radix-ui/react-dialog';
import type { ReactNode } from 'react';
import { X } from 'lucide-react';
import { cn } from './button';

// 模态弹窗：Radix Dialog 底座（焦点陷阱、Esc、遮罩点击、滚动锁、关闭后焦点归还）。
//
// 结构固定为「头 / 身 / 尾」三段：头尾各一条 hairline，只有中间那段滚动——
// 内容长的弹窗（技能部署、回收站）滚起来标题和按钮不动，用户不会失去位置感。
//
// 只做入场动画、不做退场：Radix 的 Presence 在关闭时会等 animationend，
// 若关闭态仍有 animationName（例如无条件挂 `dialog-in`），浮层会永久留在屏幕上。
// 所以动画一律挂 `data-[state=open]:`，关闭态立即卸载。
export function Dialog({
  open,
  title,
  subtitle,
  children,
  footer,
  actions,
  onClose,
  wide,
  xwide,
}: {
  open: boolean;
  title: string;
  /** 标题下一行小字：一句话说清「这个弹窗会对你做什么」，替代正文里的长解释 */
  subtitle?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  /** 头部动作：固定在标题右侧，切内容/滚动都不消失 */
  actions?: ReactNode;
  onClose: () => void;
  wide?: boolean;
  /** 数据表类弹窗：需要横向空间摆多列，又不至于占满整屏 */
  xwide?: boolean;
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
          // 无副标题时显式给空描述，避免 Radix 的「缺少 Description」警告；标题已由 Title 关联。
          aria-describedby={subtitle ? undefined : ''}
          className={cn(
            'fixed inset-0 z-40 m-auto flex h-fit max-h-[85vh] w-[calc(100%-3rem)] flex-col overflow-hidden rounded-xl border border-line bg-bg shadow-pop',
            'data-[state=open]:animate-[dialog-in_var(--dur-2)_ease-out]',
            xwide ? 'max-w-[1040px]' : wide ? 'max-w-[560px]' : 'max-w-[420px]',
          )}
        >
          <header className="flex items-start gap-3 border-b border-line px-5 py-3.5">
            <div className="min-w-0 flex-1">
              <RadixDialog.Title asChild>
                <h3 className="text-md leading-5 font-semibold">{title}</h3>
              </RadixDialog.Title>
              {subtitle && (
                <RadixDialog.Description asChild>
                  <p className="mt-1 text-xs leading-4 text-muted">{subtitle}</p>
                </RadixDialog.Description>
              )}
            </div>
            {actions && (
              <div className="flex shrink-0 flex-wrap items-center justify-end gap-1.5 self-center">
                {actions}
              </div>
            )}
            <RadixDialog.Close asChild>
              <button
                type="button"
                aria-label="关闭"
                title="关闭（Esc）"
                className="-mt-0.5 -mr-1 grid h-6 w-6 shrink-0 cursor-pointer place-items-center rounded-sm text-muted transition-colors duration-[var(--dur-1)] hover:bg-hover hover:text-fg"
              >
                <X size={14} />
              </button>
            </RadixDialog.Close>
          </header>
          <div className="min-h-0 flex-1 overflow-auto px-5 py-4">{children}</div>
          {footer && (
            <footer className="flex flex-wrap justify-end gap-2 border-t border-line px-5 py-3">
              {footer}
            </footer>
          )}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
