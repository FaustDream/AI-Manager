// 下拉选择：Radix Select 底座，样式全走令牌。
//
// 为什么换掉原生 <select>：原生弹出层由操作系统绘制——暗色风格下会弹出一块
// 刺眼的白色系统菜单，字号/圆角/滚动条都不受令牌控制，长列表没有搜索，
// 而且 34 处下拉在四套风格里长得一模一样（都是系统灰）。
//
// 对外 API 与原生刻意不同：`options` + `onValueChange`（字符串），
// 不再伪造 change 事件。迁移时把 onChange={e => f(e.target.value)} 换成 onValueChange={f}。
import * as Sel from '@radix-ui/react-select';
import { Check, ChevronDown } from 'lucide-react';
import { cn } from './button';

export interface PickerOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface PickerProps {
  value: string;
  onValueChange: (next: string) => void;
  options: PickerOption[];
  placeholder?: string;
  disabled?: boolean;
  invalid?: boolean;
  /** 读屏名称。没有可见 Field 标签时必给。 */
  label?: string;
  className?: string;
  id?: string;
  size?: 'sm' | 'md';
}

export function Picker({
  value,
  onValueChange,
  options,
  placeholder = '请选择',
  disabled,
  invalid,
  label,
  className,
  id,
  size = 'md',
}: PickerProps) {
  return (
    <Sel.Root value={value} onValueChange={onValueChange} disabled={disabled}>
      <Sel.Trigger
        id={id}
        aria-label={label}
        aria-invalid={invalid || undefined}
        className={cn(
          'inline-flex max-w-[240px] cursor-pointer items-center justify-between gap-1.5 rounded-md border border-line bg-bg',
          'text-left text-sm text-fg transition-[border-color,background-color,opacity] duration-[var(--dur-1)]',
          'hover:border-line-strong disabled:cursor-default disabled:opacity-55',
          size === 'sm' ? 'px-2 py-1 text-xs' : 'px-2.5 py-1.5',
          invalid && 'border-bad hover:border-bad',
          className,
        )}
      >
        <span className="min-w-0 truncate">
          <Sel.Value placeholder={placeholder} />
        </span>
        <Sel.Icon asChild>
          <ChevronDown size={13} aria-hidden className="shrink-0 text-muted" />
        </Sel.Icon>
      </Sel.Trigger>
      <Sel.Portal>
        <Sel.Content
          position="popper"
          sideOffset={4}
          collisionPadding={8}
          className={cn(
            'z-[65] max-h-[min(320px,var(--radix-select-content-available-height))] min-w-[var(--radix-select-trigger-width)]',
            'overflow-hidden rounded-md border border-line bg-card shadow-pop',
            'data-[state=open]:animate-[fade-in_var(--dur-2)_ease-out]',
          )}
        >
          <Sel.ScrollUpButton className="flex h-5 items-center justify-center text-muted">
            <ChevronDown size={12} className="rotate-180" aria-hidden />
          </Sel.ScrollUpButton>
          <Sel.Viewport className="p-1">
            {options.map((opt) => (
              <Sel.Item
                key={opt.value}
                value={opt.value}
                disabled={opt.disabled}
                className={cn(
                  'relative flex cursor-pointer select-none items-center gap-2 rounded-sm py-1.5 pl-6 pr-2 text-sm text-fg2 outline-none',
                  'data-[highlighted]:bg-hover data-[highlighted]:text-fg',
                  'data-[state=checked]:text-fg data-[disabled]:cursor-default data-[disabled]:opacity-45',
                )}
              >
                <span className="absolute left-1.5 flex h-3.5 w-3.5 items-center justify-center">
                  <Sel.ItemIndicator>
                    <Check size={12} aria-hidden className="text-accent" />
                  </Sel.ItemIndicator>
                </span>
                <Sel.ItemText>{opt.label}</Sel.ItemText>
              </Sel.Item>
            ))}
          </Sel.Viewport>
          <Sel.ScrollDownButton className="flex h-5 items-center justify-center text-muted">
            <ChevronDown size={12} aria-hidden />
          </Sel.ScrollDownButton>
        </Sel.Content>
      </Sel.Portal>
    </Sel.Root>
  );
}