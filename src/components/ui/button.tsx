import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

// shadcn 风格的基础组件：源码在本仓库（D6），只保留用得到的变体。
import type { ButtonHTMLAttributes } from 'react';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'default' | 'primary' | 'ghost' | 'danger';
  size?: 'sm' | 'md';
}

export function Button({ className, variant = 'default', size = 'md', ...props }: ButtonProps) {
  return (
    <button
      className={cn(
        'inline-flex items-center gap-1 rounded-md border border-line px-3 py-1 text-sm',
        'cursor-pointer hover:bg-hover disabled:cursor-default disabled:opacity-45 disabled:bg-transparent',
        variant === 'primary' && 'border-accent bg-accent text-on-accent hover:border-accent-strong hover:bg-accent-strong',
        variant === 'ghost' && 'border-transparent text-fg2 hover:text-fg',
        variant === 'danger' && 'border-bad text-bad',
        size === 'sm' && 'rounded-sm px-2 py-0.5 text-xs',
        className,
      )}
      {...props}
    />
  );
}
