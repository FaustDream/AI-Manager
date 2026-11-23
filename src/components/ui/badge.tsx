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
  // 区块**不再有卡片外观**：只有内边距，没有边框 / 底色 / 阴影 / 圆角。
  // 页面靠留白与标题分级组织内容，不再把每块内容套进一个盒子里（视觉束缚感）。
  // 列表（表格）自己保留边框——那是数据边界，不是容器装饰。
  return <div className={cn('p-4', className)} {...props} />;
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
