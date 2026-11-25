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
// 两条不能写回去的约束：
//   1. **悬停样式一律挂 `enabled:`**。主按钮（蓝底白字）在被点中的瞬间同时满足
//      「disabled」和「hover」，若用 `disabled:hover:bg-transparent` 抹掉悬停反馈，
//      底色被抹成透明而文字仍是白的——整个按钮看起来就是「空白」，白 spinner 也一起隐形。
//   2. **加载中不要再叠 `opacity-55`**。禁用态降透明度 + 文字再降一档 = 38%，
//      浅色主题下几乎读不出来。加载中保持满色，只让文字淡一档；真正禁用才 55%。
// ────────────────────────────────────────────────────────────────────────────
import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { forwardRef } from 'react';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'default' | 'primary' | 'soft' | 'ghost' | 'danger';
  size?: 'sm' | 'md' | 'lg';
  /** 加载中：出现 spinner、禁点、宽度锁定，文字保持原样 */
  loading?: boolean;
  children?: ReactNode;
}

/** spinner：默认 12px，sm 按钮用 10px（12px 在 py-0.5 的小按钮里偏重）。
 *  颜色用 currentColor，深浅主题与各变体自动跟随。 */
function Spinner({ small }: { small?: boolean }) {
  return (
    <svg
      className={`${small ? 'h-2.5 w-2.5' : 'h-3 w-3'} shrink-0 animate-spin`}
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

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant = 'default', size = 'md', loading = false, children, disabled, ...props },
  forwarded,
) {
  const busy = loading || disabled;
  return (
    <button
      // min-width 记下静止宽度：加载中多出来的 spinner 只让按钮向右长十几像素，
      // 文案本身不重排，也不会因为换了文案（例如「签到」→「签到中」）而缩回去
      ref={(el) => {
        if (typeof forwarded === 'function') forwarded(el);
        else if (forwarded) forwarded.current = el;
        if (!el) return;
        const w = el.offsetWidth;
        if (w > 0 && !el.style.minWidth) el.style.minWidth = `${w}px`;
      }}
      aria-busy={loading || undefined}
      disabled={busy}
      className={cn(
        'relative inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap rounded-md border border-line px-3 py-1 text-sm',
        // transform 一并进过渡：按下时 80ms 内下沉 1px，释放时回弹到 0（不加过冲）
        // transform 与 translate 一并进过渡：Tailwind v4 的位移类走的是 `translate`
        // 属性而不是 `transform`，只写 transform 的话悬停抬升会「啪」地跳过去
        'transition-[background-color,border-color,color,opacity,transform,translate] duration-[var(--dur-1)]',
        // 悬停上浮 1px：位移不算「特效」，是让指针下的按钮先「抬」一点再按下沉，
        // 一抬一沉凑成完整的按压触感；做不到就只剩一个色块变色
        'enabled:hover:-translate-y-px',
        // 按下反馈：只对可用状态生效。加载中再按一下会抖，用户会以为点重了
        'enabled:active:translate-y-px enabled:active:scale-[.98]',
        // 悬停只对可用状态生效：禁用/加载中不换底色，主按钮才不会「白字配透明底」
        'enabled:hover:bg-hover',
        // 默认变体也要显式给文字色：不给的话，disabled 时浏览器会套自己的灰字，
        // 再叠上文字那层 70% 就几乎看不见了
        variant === 'default' && 'text-fg',
        variant === 'primary' &&
          'border-accent bg-accent text-on-accent enabled:hover:border-accent-strong enabled:hover:bg-accent-strong',
        // 次强调：淡主色底 + 主色字。比描边按钮重、比实底主按钮轻，
        // 用在「这一区的主操作，但不是整页的主操作」
        variant === 'soft' &&
          'border-accent/25 bg-accent-soft text-accent-strong enabled:hover:border-accent enabled:hover:bg-accent enabled:hover:text-on-accent',
        variant === 'ghost' && 'border-transparent text-fg2 enabled:hover:text-fg',
        // 不可逆动作用**实底**：红框红字在一排描边按钮里读不出份量，
        // 实底 + 白字才是「这一下删掉就没了」的视觉重量。
        variant === 'danger' &&
          'border-bad bg-bad text-on-accent enabled:hover:border-bad-strong enabled:hover:bg-bad-strong',
        size === 'sm' && 'rounded-sm px-2 py-0.5 text-xs',
        size === 'lg' && 'px-4 py-2 text-md',
        // 光标与透明度按状态显式给，不靠 disabled: 变体互相覆盖
        loading ? 'cursor-progress' : disabled ? 'cursor-default opacity-55' : 'cursor-pointer',
        className,
      )}
      {...props}
    >
      {loading && <Spinner small={size === 'sm'} />}
      {/* 文字包一层：加载中只降一档不透明度，宽度与文案都不变。
          必须是 inline-flex：preflight 把 svg 设成了块级，用 inline-block 包的话
          「图标 + 文字」会被拆成上下两行——按钮看着像一张小卡片。 */}
      <span
        className={cn(
          'inline-flex items-center gap-1.5 transition-opacity duration-[var(--dur-1)]',
          loading && 'opacity-70',
        )}
      >
        {children}
      </span>
    </button>
  );
});
