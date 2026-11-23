import type { InputHTMLAttributes, ReactNode, Ref, SelectHTMLAttributes, TextareaHTMLAttributes } from 'react';
import { CircleAlert, Inbox, RotateCw } from 'lucide-react';
import { cn } from './button';

// 表单基件：输入框 / 下拉 / 多行 / 勾选 / 字段包装 / 骨架 / 空错态。
//
// 状态一律显式给，不靠浏览器默认：hover 加深边框、focus 由全局焦点环负责
// （文本输入被鼠标点中也匹配 :focus-visible，所以打字时环一定在）、
// invalid 转危险色、disabled 降透明度并换光标。
const FIELD_BASE =
  'rounded-md border border-line bg-bg px-2.5 py-1.5 text-sm text-fg placeholder:text-muted ' +
  'transition-[border-color,background-color,opacity] duration-[var(--dur-1)] ' +
  'hover:border-line-strong disabled:cursor-default disabled:opacity-55';

const invalidCls = (invalid?: boolean) => invalid && 'border-bad hover:border-bad';

export function Select({
  className,
  invalid,
  ...props
}: SelectHTMLAttributes<HTMLSelectElement> & { invalid?: boolean }) {
  return (
    <select
      className={cn(FIELD_BASE, 'max-w-[240px] cursor-pointer pr-6', invalidCls(invalid), className)}
      aria-invalid={invalid || undefined}
      {...props}
    />
  );
}

export function Input({
  className,
  invalid,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & { ref?: Ref<HTMLInputElement>; invalid?: boolean }) {
  return (
    <input
      className={cn(FIELD_BASE, invalidCls(invalid), className)}
      aria-invalid={invalid || undefined}
      {...props}
    />
  );
}

export function Textarea({
  className,
  invalid,
  ...props
}: TextareaHTMLAttributes<HTMLTextAreaElement> & { invalid?: boolean }) {
  return (
    <textarea
      className={cn(FIELD_BASE, 'min-h-[72px] w-full resize-y', invalidCls(invalid), className)}
      aria-invalid={invalid || undefined}
      {...props}
    />
  );
}

export function Checkbox({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      type="checkbox"
      className={cn('h-3.5 w-3.5 shrink-0 cursor-pointer accent-[var(--color-accent)]', className)}
      {...props}
    />
  );
}

/**
 * 字段包装：标签 + 控件 + 说明 / 错误。
 * 错误文案取代说明位（不另起一行把表单撑高），并挂 aria-describedby 让读屏读得到。
 */
export function Field({
  label,
  hint,
  error,
  htmlFor,
  children,
  className,
}: {
  label: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  htmlFor?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn('flex flex-col gap-1.5', className)}>
      <label htmlFor={htmlFor} className="text-xs font-medium text-fg2">
        {label}
      </label>
      {children}
      {error ? (
        <p className="flex items-start gap-1 text-xs text-bad-strong">
          <CircleAlert size={13} className="mt-px shrink-0" />
          <span className="min-w-0">{error}</span>
        </p>
      ) : (
        hint && <p className="text-xs text-muted">{hint}</p>
      )}
    </div>
  );
}

/** 表格骨架：只能放在 `<table><tbody>` 里（返回的是 `<tr>`／`<td>`）。 */
export function SkeletonRows({ rows = 5, cols = 4 }: { rows?: number; cols?: number }) {
  return (
    <>
      {Array.from({ length: rows }, (_, r) => (
        <tr key={r}>
          {Array.from({ length: cols }, (_, c) => (
            <td key={c} className="skeleton-in border-b border-line px-2.5 py-3">
              {/* 行间 60ms 相位差：整片同时呼吸像「一个灰块」在闪，错开才像逐行在填 */}
              <span
                className="block h-3 skeleton-block rounded-sm bg-hover"
                style={{ animationDelay: r < 6 ? `${r * 60}ms` : undefined }}
              />
            </td>
          ))}
        </tr>
      ))}
    </>
  );
}

/**
 * 非表格场景的骨架（日志行、卡片列表、对话框内容）。
 * 和 `SkeletonRows` 分开是必要的：`<tr>` 塞进 `<div>` 不会按表格渲染。
 */
export function SkeletonBlock({ rows = 4, className }: { rows?: number; className?: string }) {
  return (
    <div className={cn('skeleton-in space-y-2', className)} aria-busy="true">
      {Array.from({ length: rows }, (_, index) => (
        <div
          key={index}
          className="h-4 skeleton-block rounded-sm bg-hover"
          style={{ animationDelay: index < 6 ? `${index * 60}ms` : undefined }}
        />
      ))}
    </div>
  );
}

/** 三态之空/错误：七个列表共用同一套排版——图标 + 结论 + 下一步。 */
export function StateBlock({
  kind,
  title,
  detail,
  onRetry,
  action,
}: {
  kind: 'empty' | 'error';
  title: string;
  detail?: string;
  onRetry?: () => void;
  /** 空态的下一步动作（重新扫描 / 清空筛选…）。与错误态的「重试」并列不冲突。 */
  action?: ReactNode;
}) {
  const Icon = kind === 'error' ? CircleAlert : Inbox;
  return (
    // view-enter：空/错误态常常是「查完才知道」，直接硬切出来会让内容区跳一下
    <div className="view-enter flex flex-col items-center gap-2 px-6 py-10 text-center">
      <span
        className={cn(
          'grid h-10 w-10 place-items-center rounded-full border',
          kind === 'error' ? 'border-bad/25 bg-bad-soft text-bad' : 'border-line bg-sunken text-muted',
        )}
      >
        <Icon size={18} />
      </span>
      <p className={cn('text-sm font-medium', kind === 'error' ? 'text-bad-strong' : 'text-fg2')}>{title}</p>
      {detail && <p className="max-w-[52ch] text-xs leading-5 text-muted">{detail}</p>}
      {action && <div className="mt-1 flex flex-wrap items-center justify-center gap-2">{action}</div>}
      {kind === 'error' && onRetry && (
        <button
          type="button"
          className="mt-1 inline-flex cursor-pointer items-center gap-1.5 rounded-md border border-line bg-bg px-3 py-1 text-sm transition-colors duration-[var(--dur-1)] hover:bg-hover"
          onClick={onRetry}
        >
          <RotateCw size={13} />
          重试
        </button>
      )}
    </div>
  );
}