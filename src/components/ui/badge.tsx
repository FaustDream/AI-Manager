import { cn } from './button';
import type { HTMLAttributes, ReactNode } from 'react';

export function Tag({
  tone,
  children,
  className,
}: {
  tone?: 'ok' | 'warn' | 'bad' | 'muted';
  children: ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        'inline-block max-w-full truncate rounded-sm border px-2 py-px text-xs',
        tone === 'ok' && 'border-ok text-ok',
        tone === 'warn' && 'border-warn text-warn',
        tone === 'bad' && 'border-bad text-bad',
        (!tone || tone === 'muted') && 'border-line text-muted',
        className,
      )}
    >
      {children}
    </span>
  );
}

export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('rounded-lg border border-line bg-card p-4', className)} {...props} />;
}

export function Notice({
  tone,
  children,
  className,
}: {
  tone?: 'info' | 'warn' | 'bad';
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'whitespace-pre-wrap rounded-md border border-line bg-card p-2 text-sm',
        'border-l-[3px] border-l-accent',
        tone === 'warn' && 'border-l-warn',
        tone === 'bad' && 'border-l-bad',
        className,
      )}
    >
      {children}
    </div>
  );
}
