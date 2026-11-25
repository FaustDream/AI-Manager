// 标签页：Radix Tabs 底座。
//
// 全站原来有 6 套各写一遍的页签：手写 role=tablist + aria-selected + 自己算方向键。
// 换底座后键盘漫游、Home/End、焦点与 aria 由 Radix 负责，样式仍只从令牌取。
// 只封装「页签条」这一层：面板仍由页面条件渲染，避免为了页签改写每个页面的内容区。
import * as TabsPrimitive from '@radix-ui/react-tabs';
import type { ReactNode } from 'react';
import { cn } from './button';
import { Hint } from './tooltip';

export interface TabItem {
  value: string;
  label: ReactNode;
  /** 可选前导图标（图标由页面传入，共享层不引业务图标） */
  icon?: ReactNode;
  /** 悬停说明：解释这个视图看到什么，不占用可见文案 */
  hint?: string;
  disabled?: boolean;
}

export function Tabs({
  value,
  onValueChange,
  items,
  size = 'sm',
  variant = 'soft',
  className,
  label,
}: {
  value: string;
  onValueChange: (next: string) => void;
  items: TabItem[];
  size?: 'sm' | 'md';
  /** soft = 内容区页签；solid = 顶层分区；underline = 页内子视图 */
  variant?: 'soft' | 'solid' | 'underline';
  className?: string;
  /** 读屏名称：页签条必须有名字，否则读屏只报「标签页」 */
  label: string;
}) {
  return (
    <TabsPrimitive.Root value={value} onValueChange={onValueChange}>
      <TabsPrimitive.List
        aria-label={label}
        className={cn('flex flex-wrap items-center gap-1', className)}
      >
        {items.map((item) => {
          const trigger = (
            <TabsPrimitive.Trigger
              key={item.value}
              value={item.value}
              disabled={item.disabled}
              className={cn(
                'inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap rounded-sm border border-transparent text-fg2',
                'transition-colors duration-[var(--dur-1)]',
                'hover:bg-hover hover:text-fg',
                variant === 'solid'
                  ? 'data-[state=active]:bg-accent data-[state=active]:font-medium data-[state=active]:text-on-accent'
                  : variant === 'underline'
                    ? '-mb-px rounded-none border-x-0 border-t-0 border-b-2 data-[state=active]:border-accent data-[state=active]:font-medium data-[state=active]:text-accent'
                    : 'data-[state=active]:border-accent/25 data-[state=active]:bg-accent-soft data-[state=active]:font-medium data-[state=active]:text-accent-strong',
                'disabled:cursor-default disabled:opacity-55',
                size === 'sm' ? 'px-2 py-0.5 text-xs' : 'px-2.5 py-1 text-sm',
              )}
            >
              {item.icon}
              {item.label}
            </TabsPrimitive.Trigger>
          );
          return item.hint ? (
            <Hint key={item.value} content={item.hint} focusable={false}>
              {trigger}
            </Hint>
          ) : (
            trigger
          );
        })}
      </TabsPrimitive.List>
    </TabsPrimitive.Root>
  );
}
