import { cn } from './button';
import type { InputHTMLAttributes, SelectHTMLAttributes, TextareaHTMLAttributes } from 'react';

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

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
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

/** 三态骨架：列表加载中的占位行。 */
export function SkeletonRows({ rows = 5, cols = 4 }: { rows?: number; cols?: number }) {
  return (
    <>
      {Array.from({ length: rows }, (_, r) => (
        <tr key={r}>
          {Array.from({ length: cols }, (_, c) => (
            <td key={c} className="border-b border-line px-2.5 py-3">
              <span className="block h-3 animate-pulse rounded-sm bg-hover" />
            </td>
          ))}
        </tr>
      ))}
    </>
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
    <div className="p-8 text-center">
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
