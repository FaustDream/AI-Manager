// 仪表盘与用量共用的小块：KPI 瓦片与区块标题。
// 遵循 dashboard-design 技能的 KPI 瓦片模式：label + 大数值 + delta 徽章；
// 解释性文字一律进 tooltip（title），瓦片上只留数字与一两个词的基线。
import type { ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
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
  /*
   * 数值变化闪一档（fg → accent → fg，约 320ms）：刷新后数字换了，用户往往是
   * 先看见「这一块动了」才去读新值。只在默认色调上闪——ok/warn 是语义色，
   * 覆盖成 accent 会让「进账/告警」读不出来。
   */
  const [flash, setFlash] = useState(false);
  const prevValue = useRef(value);
  useEffect(() => {
    if (prevValue.current === value) return;
    prevValue.current = value;
    if (tone !== 'default') return;
    setFlash(true);
    const timer = window.setTimeout(() => setFlash(false), 320);
    return () => window.clearTimeout(timer);
  }, [value, tone]);

  const valueCls = cn(
    'mt-1 text-2xl font-semibold tabular-nums tracking-[-.03em] transition-colors duration-[var(--dur-2)]',
    tone === 'ok' && 'text-ok',
    tone === 'warn' && 'text-warn',
    flash && 'text-accent',
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
      <button
        type="button"
        className={cn(
          cls,
          'cursor-pointer text-left transition-[box-shadow,transform] duration-[var(--dur-1)] active:scale-[.995]',
        )}
        title={hint}
        onClick={onClick}
      >
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
    // 标题与内容留 10px（V-05 的「卡片内行距」档），此前 6px 太贴，扫读时分不清区块边界
    <div className="mb-2.5 flex items-baseline gap-2">
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
  /*
   * 首次挂载从 0 长到目标（延迟 80ms 让列表先落位），之后 pct 变化直接过渡到新值。
   * 排名条一眼要能看出「谁多谁少」，硬切的话刷新前后没有可比性。
   */
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setReady(true), 80);
    return () => window.clearTimeout(timer);
  }, []);
  const target = Math.max(1.5, Math.min(100, pct));
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
          'absolute inset-y-0 left-0 rounded-full opacity-75 transition-[width,opacity] duration-[var(--dur-4)] ease-[var(--ease-std)] group-hover:opacity-100',
          tone === 'bad' ? 'bg-bad' : 'bg-accent',
        )}
        style={{ width: ready ? `${target}%` : '0%' }}
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
