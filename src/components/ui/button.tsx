import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

// shadcn 风格的基础组件：源码在本仓库（D6），只保留用得到的变体。
//
// ── 按钮加载态的设计约束（refactoring-ui 审查 brief）────────────────────────
// 内容分级：按钮文字 = 唯一身份载体，加载中**不替换文字**（替换会让宽度跳变、
// 工具栏回流抖动）；层级由「spinner 出现 + 光标变进度 + 文字降到 70%」表达，
// 不靠加噪（放大/变色/加边框）。
//
// 两条踩过的坑，别再写回去：
//   1. **悬停样式一律挂 `enabled:`**。曾经用 `disabled:hover:bg-transparent` 抹掉
//      悬停反馈，结果主按钮（蓝底白字）在被点中的瞬间同时满足「disabled」和
//      「hover」，底色被抹成透明而文字仍是白的——整个按钮看起来就是「空白」，
//      白 spinner 也一起隐形。
//   2. **加载中不要再叠 `opacity-55`**。禁用态降透明度 + 文字再降一档 = 38%，
//      浅色主题下几乎读不出来。加载中保持满色，只让文字淡一档；真正禁用才 55%。
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
      // min-width 记下静止宽度：加载中多出来的 spinner 只让按钮向右长十几像素，
      // 文案本身不重排，也不会因为换了文案（例如「签到」→「签到中」）而缩回去
      ref={(el) => {
        if (!el) return;
        const w = el.offsetWidth;
        if (w > 0 && !el.style.minWidth) el.style.minWidth = `${w}px`;
      }}
      aria-busy={loading || undefined}
      disabled={busy}
      className={cn(
        'relative inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-md border border-line px-3 py-1 text-sm',
        'transition-[background-color,border-color,color,opacity] duration-150',
        // 悬停只对可用状态生效：禁用/加载中不换底色，主按钮才不会「白字配透明底」
        'enabled:hover:bg-hover',
        // 默认变体也要显式给文字色：不给的话，disabled 时浏览器会套自己的灰字，
        // 再叠上文字那层 70% 就几乎看不见了
        variant === 'default' && 'text-fg',
        variant === 'primary' &&
          'border-accent bg-accent text-on-accent enabled:hover:border-accent-strong enabled:hover:bg-accent-strong',
        variant === 'ghost' && 'border-transparent text-fg2 enabled:hover:text-fg',
        variant === 'danger' && 'border-bad text-bad',
        size === 'sm' && 'rounded-sm px-2 py-0.5 text-xs',
        // 光标与透明度按状态显式给，不靠 disabled: 变体互相覆盖
        loading ? 'cursor-progress' : disabled ? 'cursor-default opacity-55' : 'cursor-pointer',
        className,
      )}
      {...props}
    >
      {loading && <Spinner />}
      {/* 文字包一层：加载中只降一档不透明度，宽度与文案都不变 */}
      <span className={cn('inline-block transition-opacity duration-150', loading && 'opacity-70')}>
        {children}
      </span>
    </button>
  );
}
