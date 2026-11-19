import { cn } from './button';
import type { InputHTMLAttributes, Ref, SelectHTMLAttributes, TextareaHTMLAttributes } from 'react';

export function Select({ className, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      className={cn(
        'rounded-sm border border-line bg-bg px-2 py-1 text-sm max-w-[220px]',
        className,
      )}
      {...props}
    />
  );
}

export function Input({
  className,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & { ref?: Ref<HTMLInputElement> }) {
  return (
    <input
      className={cn('rounded-sm border border-line bg-bg px-2 py-1 text-sm', className)}
      {...props}
    />
  );
}

export function Textarea({ className, ...props }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(
        'min-h-[72px] w-full resize-y rounded-sm border border-line bg-bg px-2 py-1 text-sm',
        className,
      )}
      {...props}
    />
  );
}

export function Checkbox({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return <input type="checkbox" className={cn('accent-[var(--color-accent)]', className)} {...props} />;
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
 *
 * 和 `SkeletonRows` 分开是必要的：`<tr>` 塞进 `<div>` 不会按表格渲染，
 * 浏览器只画出几个行盒——看起来就是「加载时一片空白」，比不放骨架还糟。
 */
export function SkeletonBlock({
  rows = 4,
  className,
}: {
  rows?: number;
  className?: string;
}) {
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

/** 三态之空/错误：七个列表共用同一套排版（v1 同款）。 */
export function StateBlock({
  kind,
  title,
  detail,
  onRetry,
}: {
  kind: 'empty' | 'error';
  title: string;
  detail?: string;
  onRetry?: () => void;
}) {
  return (
    // view-enter：空/错误态常常是「查完才知道」，直接硬切出来会让内容区跳一下
    <div className="view-enter p-8 text-center">
      <p className={`text-sm ${kind === 'error' ? 'text-bad' : 'text-fg2'}`}>{title}</p>
      {detail && <p className="mt-1 text-xs text-muted">{detail}</p>}
      {kind === 'error' && onRetry && (
        <button
          className="mt-3 rounded-md border border-line px-3 py-1 text-sm hover:bg-hover"
          onClick={onRetry}
        >
          重试
        </button>
      )}
    </div>
  );
}
