// 会话行的右键菜单（轻量浮层）。原生 contextmenu 已被全局守卫拦掉
// （lib/context-menu.ts），这里的浮层就是「右键 = 干这行的常用动作」的载体。
// 定位在指针处，点击任意项 / Esc / 滚动 / 点击遮罩即关闭。
import { useEffect, useRef } from 'react';
import { cn } from '@/components/ui/button';

export interface MenuItem {
  label: string;
  /** 危险动作（红色 + 需要确认的语义在 onSelect 里做） */
  danger?: boolean;
  disabled?: boolean;
  /** 右侧灰字：解释为什么灰着 / 动作的补充说明 */
  hint?: string;
  onSelect: () => void;
}

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);

  // 贴边：菜单超出行视口就往回挪，保证完整可见
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const rect = node.getBoundingClientRect();
    const flipX = x + rect.width > window.innerWidth - 8;
    const flipY = y + rect.height > window.innerHeight - 8;
    if (flipX) node.style.left = `${Math.max(8, x - rect.width)}px`;
    if (flipY) node.style.top = `${Math.max(8, y - rect.height)}px`;
  }, [x, y]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  if (items.length === 0) return null;
  return (
    <div
      className="fixed inset-0 z-40"
      onMouseDown={onClose}
      onContextMenu={(e) => {
        e.preventDefault();
        onClose();
      }}
      onWheel={onClose}
    >
      <div
        ref={ref}
        role="menu"
        className="absolute min-w-[168px] overflow-hidden rounded-lg border border-line bg-card py-1 shadow-[0_8px_28px_-8px_rgba(16,24,40,.35)]"
        style={{ left: x, top: y }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {items.map((item) => (
          <button
            key={item.label}
            role="menuitem"
            disabled={item.disabled}
            className={cn(
              'flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left text-sm',
              item.disabled ? 'text-muted/60' : 'hover:bg-hover',
              item.danger && !item.disabled && 'text-bad',
            )}
            title={item.hint}
            onClick={() => {
              onClose();
              item.onSelect();
            }}
          >
            <span className="min-w-0 flex-1 truncate">{item.label}</span>
            {item.hint && <span className="text-xs text-muted">{item.disabled ? item.hint : ''}</span>}
          </button>
        ))}
      </div>
    </div>
  );
}
