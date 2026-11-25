// 菜单：Radix DropdownMenu 底座。右键上下文菜单与按钮菜单共用一套。
//
// 为什么换掉手写 ContextMenu：手写版要自己算指针位置、边缘翻转、Esc/滚动/遮罩
// 关闭、焦点回归，还要自己接 role=menu/menuitem 的键盘漫游。Radix 把这些做全了，
// 而且碰撞翻转由 Floating UI 负责，贴近视口边缘时不会翻出屏幕。
import * as Menu from '@radix-ui/react-dropdown-menu';
import type { ReactNode } from 'react';
import { cn } from './button';

export interface MenuEntry {
  key: string;
  label: string;
  icon?: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  danger?: boolean;
  /** 右侧快捷键提示 */
  shortcut?: string;
}

export function DropdownMenu({
  trigger,
  entries,
  align = 'end',
  side = 'bottom',
  label,
}: {
  trigger: ReactNode;
  entries: (MenuEntry | 'separator')[];
  align?: 'start' | 'center' | 'end';
  side?: 'top' | 'right' | 'bottom' | 'left';
  label: string;
}) {
  return (
    <Menu.Root>
      <Menu.Trigger asChild>{trigger}</Menu.Trigger>
      <Menu.Portal>
        <Menu.Content
          align={align}
          side={side}
          sideOffset={4}
          collisionPadding={8}
          aria-label={label}
          className={cn(
            'z-[65] min-w-[168px] overflow-hidden rounded-md border border-line bg-card p-1 shadow-pop',
            'data-[state=open]:animate-[fade-in_var(--dur-2)_ease-out]',
          )}
        >
          {entries.map((entry, i) =>
            entry === 'separator' ? (
              <Menu.Separator key={`sep-${i}`} className="my-1 h-px bg-line" />
            ) : (
              <Menu.Item
                key={entry.key}
                disabled={entry.disabled}
                onSelect={entry.onSelect}
                className={cn(
                  'flex cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none',
                  'data-[highlighted]:bg-hover',
                  entry.danger ? 'text-bad data-[highlighted]:bg-bad-soft' : 'text-fg2 data-[highlighted]:text-fg',
                  'data-[disabled]:cursor-default data-[disabled]:opacity-45',
                )}
              >
                {entry.icon && <span className="flex h-3.5 w-3.5 shrink-0 items-center justify-center">{entry.icon}</span>}
                <span className="min-w-0 flex-1 truncate">{entry.label}</span>
                {entry.shortcut && <span className="shrink-0 text-xs text-muted">{entry.shortcut}</span>}
              </Menu.Item>
            ),
          )}
        </Menu.Content>
      </Menu.Portal>
    </Menu.Root>
  );
}

/**
 * 右键菜单：在任意区域上右键弹出。`children` 是被包裹的区域。
 * 位置由 Radix 的虚拟触发点接管，不用自己算 clientX/clientY。
 */
export function ContextMenu({
  children,
  entries,
  label,
  disabled,
}: {
  children: ReactNode;
  entries: (MenuEntry | 'separator')[];
  label: string;
  disabled?: boolean;
}) {
  if (disabled || entries.length === 0) return <>{children}</>;
  return (
    <Menu.Root>
      <Menu.Trigger asChild onContextMenu={(e) => e.preventDefault()}>
        <div>{children}</div>
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Content
          sideOffset={2}
          collisionPadding={8}
          aria-label={label}
          className={cn(
            'z-[65] min-w-[168px] overflow-hidden rounded-md border border-line bg-card p-1 shadow-pop',
            'data-[state=open]:animate-[fade-in_var(--dur-2)_ease-out]',
          )}
        >
          {entries.map((entry, i) =>
            entry === 'separator' ? (
              <Menu.Separator key={`sep-${i}`} className="my-1 h-px bg-line" />
            ) : (
              <Menu.Item
                key={entry.key}
                disabled={entry.disabled}
                onSelect={entry.onSelect}
                className={cn(
                  'flex cursor-pointer select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none',
                  'data-[highlighted]:bg-hover',
                  entry.danger ? 'text-bad data-[highlighted]:bg-bad-soft' : 'text-fg2 data-[highlighted]:text-fg',
                  'data-[disabled]:cursor-default data-[disabled]:opacity-45',
                )}
              >
                {entry.icon && <span className="flex h-3.5 w-3.5 shrink-0 items-center justify-center">{entry.icon}</span>}
                <span className="min-w-0 flex-1 truncate">{entry.label}</span>
                {entry.shortcut && <span className="shrink-0 text-xs text-muted">{entry.shortcut}</span>}
              </Menu.Item>
            ),
          )}
        </Menu.Content>
      </Menu.Portal>
    </Menu.Root>
  );
}