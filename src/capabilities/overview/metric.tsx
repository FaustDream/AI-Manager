// 仪表盘与用量共用的小块：KPI 瓦片与区块标题。
// 遵循 dashboard-design 技能的 KPI 瓦片模式：label + 大数值 + delta 徽章；
// 解释性文字一律进 tooltip（title），瓦片上只留数字与一两个词的基线。
import type { ReactNode } from 'react';
import { cn } from '@/components/ui/button';
import { fmtTokensExact } from '@/lib/format';

/** 与昨日等长窗口对比的 delta 徽章。Token 用量是中性量：涨跌都不带红绿，只给方向。 */
export function Delta({ cur, prev }: { cur: number; prev: number }) {
  if (!(prev > 0)) return null;
  const r = Math.round(((cur - prev) / prev) * 100);
  const text =
    Math.abs(r) > 999 ? (r > 0 ? '10×+' : '-99%') : r === 0 ? '±0%' : `${r > 0 ? '▲' : '▼'} ${Math.abs(r)}%`;
  return (
    <span className="text-xs tabular-nums text-muted" title={`昨日: ${fmtTokensExact(prev)}`}>
      {text}
      <span className="ml-1 text-muted">vs 昨日</span>
    </span>
  );
}

export function Metric({
  label,
  value,
  exact,
  hint,
  delta,
  tone = 'default',
  onClick,
}: {
  label: string;
  value: string;
  /** 换算成亿/万后的小数有取舍，悬浮给原数 */
  exact?: string;
  /** 解释与口径：进 tooltip，不上屏 */
  hint?: string;
  delta?: ReactNode;
  /** 数值语义色：进账用 ok、临期告警用 warn；默认不着色 */
  tone?: 'default' | 'ok' | 'warn';
  /** 给了就是可点瓦片（整块是按钮），用于「点数字去处理」的入口 */
  onClick?: () => void;
}) {
  const valueCls = cn(
    'mt-1 text-2xl font-semibold tabular-nums tracking-[-.03em]',
    tone === 'ok' && 'text-ok',
    tone === 'warn' && 'text-warn',
  );
  const body = (
    <>
      <div className="flex items-center justify-between gap-2 text-xs text-fg2">
        {label}
        {delta}
      </div>
      <div className={valueCls} title={exact}>
        {value}
      </div>
    </>
  );
  const cls =
    'rounded-lg border border-line bg-card p-4 shadow-card transition-shadow hover:shadow-card-hover';
  if (onClick) {
    return (
      <button type="button" className={cn(cls, 'cursor-pointer text-left')} title={hint} onClick={onClick}>
        {body}
      </button>
    );
  }
  return (
    <div className={cls} title={hint}>
      {body}
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

/** 排名行的 3px 份额条：贴行底边，hover 提亮（dashboard-design 技能的 ranked row）。
 *  tone：accent = Token 类，bad = 消耗/支出类（颜色跟实体走）。 */
export function ShareBar({
  pct,
  tone = 'accent',
  className,
}: {
  pct: number;
  tone?: 'accent' | 'bad';
  className?: string;
}) {
  return (
    <span
      aria-hidden
      className={cn(
        'absolute inset-x-3 bottom-[5px] h-[3px] overflow-hidden rounded-full bg-line/70',
        className,
      )}
    >
      <span
        className={cn(
          'absolute inset-y-0 left-0 rounded-full opacity-75 transition-opacity group-hover:opacity-100',
          tone === 'bad' ? 'bg-bad' : 'bg-accent',
        )}
        style={{ width: `${Math.max(1.5, Math.min(100, pct))}%` }}
      />
    </span>
  );
}

/** 占比文案：真实但极小的份额不显示 0.0%。 */
export function shareText(share: number): string {
  if (share === 0) return '0%';
  if (share < 0.1) return '<0.1%';
  return `${share < 1 ? share.toFixed(1) : Math.round(share)}%`;
}
