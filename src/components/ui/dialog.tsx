import type { ReactNode } from 'react';
import { X } from 'lucide-react';
import { cn } from './button';
import { useModal } from './use-modal';

// 模态弹窗：Esc 可关，点背景也可关。
//
// 结构固定为「头 / 身 / 尾」三段：头尾各一条 hairline，只有中间那段滚动——
// 内容长的弹窗（技能部署、回收站）滚起来标题和按钮不动，用户不会失去位置感。
// 焦点纪律与退场时序在 useModal 里，与抽屉共用同一份。
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
  const { panel, visible, closing } = useModal(open, onClose);
  if (!visible) return null;
  return (
    <div
      className={cn(
        'fixed inset-0 z-40 flex items-center justify-center bg-scrim p-6',
        closing ? 'overlay-out' : 'overlay-in',
      )}
      onMouseDown={(event) => {
        // 退场中的这一层已经不再「活着」，再点一下不该重复关（会二次归还焦点）
        if (closing) return;
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={cn(
          // 面板：淡入 + 上移 4px + 97%→100% 缩放。遮罩只淡不位移，两层都位移会「抖两下」
          closing ? 'view-exit' : 'dialog-in',
          'flex max-h-[85vh] w-full flex-col overflow-hidden rounded-xl border border-line bg-bg shadow-pop',
          xwide ? 'max-w-[1040px]' : wide ? 'max-w-[560px]' : 'max-w-[420px]',
        )}
      >
        <header className="flex items-start gap-3 border-b border-line px-5 py-3.5">
          <div className="min-w-0 flex-1">
            <h3 className="text-md leading-5 font-semibold">{title}</h3>
            {subtitle && <p className="mt-1 text-xs leading-4 text-muted">{subtitle}</p>}
          </div>
          {actions && (
            <div className="flex shrink-0 flex-wrap items-center justify-end gap-1.5 self-center">
              {actions}
            </div>
          )}
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭"
            title="关闭（Esc）"
            className="-mt-0.5 -mr-1 grid h-6 w-6 shrink-0 cursor-pointer place-items-center rounded-sm text-muted transition-colors duration-[var(--dur-1)] hover:bg-hover hover:text-fg"
          >
            <X size={14} />
          </button>
        </header>
        <div className="min-h-0 flex-1 overflow-auto px-5 py-4">{children}</div>
        {footer && (
          <footer className="flex flex-wrap justify-end gap-2 border-t border-line px-5 py-3">
            {footer}
          </footer>
        )}
      </div>
    </div>
  );
}