import { AlertTriangle, Info, OctagonAlert } from 'lucide-react';
import type { ReactNode } from 'react';
import { cn } from './button';
import { Hint } from './tooltip';

/**
 * 状态标签。默认软底（淡色填充 + 同色细边），比纯描边更容易在一行密排文字里被扫到。
 * `variant="outline"` 留给需要克制的场合（极简风的表格内联标记）。
 *
 * 语义色只表达状态，不当装饰：ok 成功 / warn 需要注意 / bad 失败或不可逆 /
 * info 说明 / accent 当前项 / muted 无状态。
 */
export function Tag({
  tone,
  variant = 'soft',
  dot,
  children,
  className,
  title,
}: {
  tone?: 'ok' | 'warn' | 'bad' | 'muted' | 'accent' | 'info';
  variant?: 'soft' | 'outline';
  /** 前置状态点：只有颜色不足以区分时（同一行多个同色标签）加它 */
  dot?: boolean;
  children: ReactNode;
  className?: string;
  title?: string;
}) {
  const soft: Record<string, string> = {
    ok: 'border-ok/25 bg-ok-soft text-ok',
    warn: 'border-warn/25 bg-warn-soft text-warn',
    bad: 'border-bad/25 bg-bad-soft text-bad-strong',
    accent: 'border-accent/25 bg-accent-soft text-accent-strong',
    info: 'border-info/25 bg-info-soft text-info',
    muted: 'border-line bg-sunken text-fg2',
  };
  const outline: Record<string, string> = {
    ok: 'border-ok/45 text-ok',
    warn: 'border-warn/45 text-warn',
    bad: 'border-bad/45 text-bad-strong',
    accent: 'border-accent/45 text-accent-strong',
    info: 'border-info/45 text-info',
    muted: 'border-line text-muted',
  };
  const key = tone ?? 'muted';
  const dotColor: Record<string, string> = {
    ok: 'bg-ok',
    warn: 'bg-warn',
    bad: 'bg-bad',
    accent: 'bg-accent',
    info: 'bg-info',
    muted: 'bg-muted',
  };
  return (
    <Hint content={title} focusable={false}>
      <span
        className={cn(
          'inline-flex max-w-full items-center gap-1 truncate rounded-sm border px-1.5 py-px text-xs leading-5',
          (variant === 'outline' ? outline : soft)[key],
          className,
        )}
      >
        {dot && <span aria-hidden className={cn('h-1.5 w-1.5 shrink-0 rounded-full', dotColor[key])} />}
        <span className="min-w-0 truncate">{children}</span>
      </span>
    </Hint>
  );
}

/** 提示条：左侧 3px 语义色竖条 + 图标 + 正文。三种语气共用一套排版。 */
export function Notice({
  tone = 'info',
  children,
  className,
}: {
  tone?: 'info' | 'warn' | 'bad';
  children: ReactNode;
  className?: string;
}) {
  const Icon = tone === 'bad' ? OctagonAlert : tone === 'warn' ? AlertTriangle : Info;
  return (
    <div
      className={cn(
        'flex gap-2 rounded-md border border-line border-l-[3px] bg-card p-2.5 text-sm shadow-card',
        tone === 'info' && 'border-l-accent',
        tone === 'warn' && 'border-l-warn',
        tone === 'bad' && 'border-l-bad',
        className,
      )}
    >
      <Icon
        size={15}
        className={cn(
          'mt-0.5 shrink-0',
          tone === 'info' && 'text-accent',
          tone === 'warn' && 'text-warn',
          tone === 'bad' && 'text-bad',
        )}
      />
      <div className="min-w-0 flex-1 whitespace-pre-wrap">{children}</div>
    </div>
  );
}