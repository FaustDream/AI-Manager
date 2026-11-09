// 仪表盘与用量共用的小块：指标卡与区块标题。
// 两页合并后必须只有一套实现——同一个数字在两处长得不一样，会被当成两个指标。
import type { ReactNode } from 'react';
import { cn } from '@/components/ui/button';

export function Metric({
  label,
  value,
  hint,
  exact,
  tone,
  onClick,
}: {
  label: string;
  value: string;
  hint: string;
  /** 换算成亿/万后的小数有取舍，悬浮给原数 */
  exact?: string;
  tone?: 'ok' | 'warn' | 'bad';
  onClick?: () => void;
}) {
  return (
    <div
      role={onClick ? 'button' : undefined}
      tabIndex={onClick ? 0 : undefined}
      onClick={onClick}
      onKeyDown={(e) => {
        if (onClick && (e.key === 'Enter' || e.key === ' ')) {
          e.preventDefault();
          onClick();
        }
      }}
      className={cn(
        'rounded-lg border border-line bg-card p-3',
        onClick && 'cursor-pointer transition-colors hover:border-accent/60 hover:bg-hover',
      )}
    >
      <div className="text-xs text-muted">{label}</div>
      <div
        className={cn(
          'mt-1 text-2xl font-semibold tabular-nums',
          tone === 'ok' && 'text-ok',
          tone === 'warn' && 'text-warn',
          tone === 'bad' && 'text-bad',
        )}
        title={exact}
      >
        {value}
      </div>
      <div className="mt-0.5 truncate text-xs text-muted" title={hint}>
        {hint}
      </div>
    </div>
  );
}

export function SectionTitle({
  children,
  right,
}: {
  children: ReactNode;
  right?: ReactNode;
}) {
  return (
    <div className="mb-1.5 flex items-baseline gap-2">
      <h3 className="text-xs font-semibold tracking-wide text-fg2">{children}</h3>
      <span className="flex-1" />
      {right && <span className="text-xs text-muted">{right}</span>}
    </div>
  );
}
