import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

// shadcn 风格的基础组件：源码在本仓库（D6），只保留用得到的变体。
//
// ── 按钮加载态的设计约束（refactoring-ui 审查 brief）────────────────────────
// 内容分级：按钮文字 = 唯一身份载体，加载中**不替换文字**（替换会让宽度跳变、
// 工具栏回流抖动）；层级由「spinner 出现 + 透明度降为 55% + 光标禁用」表达，
// 不靠加噪（放大/变色/加边框）。宽度稳定用 inline min-width 锁定（两次渲染间
// 不跳动），spinner 12px 与 13px 文字基线居中。
// ────────────────────────────────────────────────────────────────────────────
import type { ButtonHTMLAttributes, ReactNode } from 'react';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'default' | 'primary' | 'ghost' | 'danger';
  size?: 'sm' | 'md';
  /** 加载中：出现 spinner、禁点、宽度锁定，文字保持原样 */
  loading?: boolean;
  children?: ReactNode;
}

/** 12px spinner：用令牌色 currentColor，深浅主题自动跟随。 */
function Spinner() {
  return (
    <svg
      className="h-3 w-3 shrink-0 animate-spin"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
    >
      <circle cx="12" cy="12" r="10" stroke="currentColor" strokeOpacity="0.25" strokeWidth="4" />
      <path
        d="M22 12a10 10 0 0 0-10-10"
        stroke="currentColor"
        strokeWidth="4"
        strokeLinecap="round"
      />
    </svg>
  );
}

export function Button({
  className,
  variant = 'default',
  size = 'md',
  loading = false,
  children,
  disabled,
  ...props
}: ButtonProps) {
  const busy = loading || disabled;
  return (
    <button
      // min-width 在渲染期把「无 spinner → 有 spinner」的宽度差锁平：
      // ref 读一次自然宽度，busy 期间固定，不会因为文字不同而抖动
      ref={(el) => {
        if (!el) return;
        const w = el.offsetWidth;
        if (w > 0 && !el.style.minWidth) el.style.minWidth = `${w}px`;
      }}
      aria-busy={loading || undefined}
      disabled={busy}
      className={cn(
        'relative inline-flex items-center justify-center gap-1.5 rounded-md border border-line px-3 py-1 text-sm',
        'transition-[background-color,border-color,opacity] duration-100',
        'cursor-pointer hover:bg-hover',
        'disabled:cursor-default disabled:opacity-55 disabled:hover:bg-transparent',
        variant === 'primary' &&
          'border-accent bg-accent text-on-accent hover:border-accent-strong hover:bg-accent-strong',
        variant === 'ghost' && 'border-transparent text-fg2 hover:text-fg',
        variant === 'danger' && 'border-bad text-bad',
        size === 'sm' && 'rounded-sm px-2 py-0.5 text-xs',
        className,
      )}
      {...props}
    >
      {loading && <Spinner />}
      {/* 文字包一层：busy 时降不透明度而不是改文字，宽度不变 */}
      <span className={cn('inline-block', loading && 'opacity-70')}>{children}</span>
    </button>
  );
}
