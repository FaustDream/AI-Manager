import { cn } from './button';
import type { HTMLAttributes, ReactNode } from 'react';

export function Tag({
  tone,
  children,
  className,
  title,
}: {
  tone?: 'ok' | 'warn' | 'bad' | 'muted' | 'accent';
  children: ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <span
      className={cn(
        'inline-block max-w-full truncate rounded-sm border px-2 py-px text-xs',
        tone === 'ok' && 'border-ok text-ok',
        tone === 'warn' && 'border-warn text-warn',
        tone === 'bad' && 'border-bad text-bad',
        tone === 'accent' && 'border-accent text-accent',
        (!tone || tone === 'muted') && 'border-line text-muted',
        className,
      )}
      title={title}
    >
      {children}
    </span>
  );
}

export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  // 区块卡片的唯一外观：圆角 + hairline + card 底 + 一档阴影。
  // 阴影两档（静置 / 悬浮）由 index.css 令牌给值，深浅主题各自覆盖。
  return (
    <div className={cn('rounded-lg border border-line bg-card p-4 shadow-card', className)} {...props} />
  );
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
