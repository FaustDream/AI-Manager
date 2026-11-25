// 开关：Radix Switch 底座。
//
// 为什么不用勾选框当开关：勾选框表达「选中这一项」，开关表达「这个功能开/关」，
// 语义不同、读屏播报也不同（role=switch + aria-checked）。设置页原来用
// Checkbox 承担功能开关，读屏只会说「复选框 未勾选」，听不出是个开关。
import * as Sw from '@radix-ui/react-switch';
import { cn } from './button';
import { Hint } from './tooltip';

export interface SwitchProps {
  checked: boolean;
  onCheckedChange: (next: boolean) => void;
  disabled?: boolean;
  /** 读屏名称。图标化开关必给。 */
  label: string;
  /** 悬停说明：为什么可以关、关掉会怎样。禁用态必须给，说清为什么不能点。 */
  hint?: string;
  size?: 'sm' | 'md';
  className?: string;
  id?: string;
}

export function Switch({
  checked,
  onCheckedChange,
  disabled,
  label,
  hint,
  size = 'md',
  className,
  id,
}: SwitchProps) {
  const dims = size === 'sm' ? 'h-4 w-7' : 'h-5 w-9';
  const knob = size === 'sm' ? 'h-3 w-3' : 'h-4 w-4';
  const shift = size === 'sm' ? 'data-[state=checked]:translate-x-3' : 'data-[state=checked]:translate-x-4';
  return (
    <Hint content={hint} focusable={false}>
      <Sw.Root
        id={id}
        checked={checked}
        onCheckedChange={onCheckedChange}
        disabled={disabled}
        aria-label={label}
        className={cn(
          'relative inline-flex shrink-0 cursor-pointer items-center rounded-full border border-line',
          'transition-colors duration-[var(--dur-1)]',
          'data-[state=checked]:border-accent data-[state=checked]:bg-accent data-[state=unchecked]:bg-sunken',
          'hover:data-[state=unchecked]:border-line-strong',
          'disabled:cursor-default disabled:opacity-55',
          dims,
          className,
        )}
      >
        <Sw.Thumb
          className={cn(
            'pointer-events-none block translate-x-0.5 rounded-full bg-card shadow-card',
            'transition-transform duration-[var(--dur-1)]',
            'data-[state=checked]:bg-on-accent',
            knob,
            shift,
          )}
        />
      </Sw.Root>
    </Hint>
  );
}