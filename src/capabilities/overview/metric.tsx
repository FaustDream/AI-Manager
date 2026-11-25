// 仪表盘与用量共用的小块：KPI 瓦片与区块标题。
// 遵循 dashboard-design 技能的 KPI 瓦片模式：label + 大数值 + delta 徽章；
// 解释性文字一律进 tooltip（title），瓦片上只留数字与一两个词的基线。
import type { PointerEvent as ReactPointerEvent, ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
import { cn } from '@/components/ui/button';
import { Hint, Tooltip } from '@/components/ui';

/*
 * 数值滚动（count-up）：只在 KPI 瓦片上做，400ms 三次缓出。
 * 表格里一律不做——一排数字各自滚动，读数就变成了猜数。
 * reduced-motion 下直接给终值，连一帧都不滚。
 */
const NO_MOTION =
  typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

function CountUp({ raw, format }: { raw: number; format: (n: number) => string }) {
  const [shown, setShown] = useState(raw);
  const from = useRef(raw);
  useEffect(() => {
    const start = from.current;
    if (start === raw) return;
    if (NO_MOTION) {
      from.current = raw;
      setShown(raw);
      return;
    }
    const t0 = performance.now();
    let raf = 0;
    const step = (now: number) => {
      const p = Math.min(1, (now - t0) / 400);
      const eased = 1 - Math.pow(1 - p, 3);
      setShown(start + (raw - start) * eased);
      if (p < 1) raf = requestAnimationFrame(step);
      else from.current = raw;
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [raw]);
  return <>{format(shown)}</>;
}

/** 指针跟随时瓦片的最大倾角。2.5° 是分界线：再大就不像仪表，像玩具。 */
const TILT_MAX = 2.5;

export function Metric({
  label,
  value,
  raw,
  format,
  exact,
  hint,
  delta,
  tone = 'default',
  onClick,
}: {
  label: string;
  value: string;
  /** 给了它 + `format` 就走数值滚动；字符串 value 只用于非数字型 KPI（如「无」） */
  raw?: number;
  format?: (n: number) => string;
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

  // 指针跟随的极轻倾斜：只有 KPI 瓦片，指针离开即回正，按压时同时微缩。
  const [tilt, setTilt] = useState('');
  const [pressed, setPressed] = useState(false);
  const onMove = (e: ReactPointerEvent<HTMLElement>) => {
    if (NO_MOTION || e.pointerType !== 'mouse') return;
    const rect = e.currentTarget.getBoundingClientRect();
    const nx = (e.clientX - rect.left) / rect.width - 0.5;
    const ny = (e.clientY - rect.top) / rect.height - 0.5;
    setTilt(
      `perspective(700px) rotateX(${(-ny * TILT_MAX).toFixed(2)}deg) ` +
        `rotateY(${(nx * TILT_MAX).toFixed(2)}deg) translateY(-1px)`,
    );
  };
  const transform = `${tilt}${pressed ? ' scale(.995)' : ''}`.trim();

  const display = raw !== undefined && format ? <CountUp raw={raw} format={format} /> : value;

  const valueCls = cn(
    'mt-1.5 text-3xl font-semibold tabular-nums tracking-[-.03em] transition-colors duration-[var(--dur-2)]',
    tone === 'ok' && 'text-ok',
    tone === 'warn' && 'text-warn',
    flash && 'text-accent',
  );
  const body = (
    <>
      <div className="flex items-center justify-between gap-2 text-xs text-fg2">
        {/* 瓦片只占半栏时，自定义区间的标签会顶到 delta 上：截断比撑破卡片好读 */}
        <span className="min-w-0 truncate">{label}</span>
        {delta}
      </div>
      <div className={valueCls}>
        <Hint content={exact} focusable={false}>{display}</Hint>
      </div>
    </>
  );
  /*
   * 瓦片是卡片里的「凹槽」：底色比卡面深一档（sunken），不描边、不投影。
   * 三层中性色 canvas → card → sunken 把页面深度做出来，同时避免卡中卡的边框噪声。
   * 可点瓦片 hover 时提亮到 hover 档，让「这个数字能点」看得见。
   */
  const cls = cn(
    'rounded-md bg-sunken p-4 transition-[transform,background-color] duration-[var(--dur-2)]',
    onClick && 'hover:bg-hover',
  );
  const motion = {
    onPointerMove: onMove,
    onPointerLeave: () => {
      setTilt('');
      setPressed(false);
    },
    onPointerDown: () => setPressed(true),
    onPointerUp: () => setPressed(false),
    style: transform ? { transform } : undefined,
  };
  if (onClick) {
    return (
      <Tooltip content={hint}>
        <button type="button" className={cn(cls, 'cursor-pointer text-left')} onClick={onClick} {...motion}>
          {body}
        </button>
      </Tooltip>
    );
  }
  return (
    <Tooltip content={hint}>
      <div className={cls} {...motion}>
        {body}
      </div>
    </Tooltip>
  );
}

export function SectionTitle({
  children,
  right,
  className,
}: {
  children: ReactNode;
  right?: ReactNode;
  /** 卡内已经有 `gap-3` 时传 `mb-0`：标题与内容的间距交给卡片的栅格，别叠成 22px */
  className?: string;
}) {
  return (
    // 卡标题左侧一枚 accent 短竖：四套风格换肤时它跟着变，是卡片的身份标记
    <div className={cn('mb-3 flex items-center gap-2', className)}>
      <span aria-hidden className="h-3.5 w-[3px] shrink-0 rounded-full bg-accent" />
      <h3 className="text-lg font-semibold tracking-tight text-fg">{children}</h3>
      <span className="flex-1" />
      {right && <span className="text-xs text-muted">{right}</span>}
    </div>
  );
}

/**
 * 分段控件：仪表盘里所有「区间 / 视图」切换共用这一套外观。
 * 外框 + 内钮 + accent 选中，与日志页「时间窗」同一口径：
 * 同一个页面上不用裸文字按钮与浅底 hover 混出两种选中态，看着像两个产品的控件。
 */
export function Segmented({
  value,
  options,
  onPick,
  title,
}: {
  value: string;
  options: { key: string; label: string; title?: string }[];
  onPick: (key: string) => void;
  /** 整组控件的说明：进外层 title，单个按钮的说明走 options[].title */
  title?: string;
}) {
  return (
    <Hint content={title} focusable={false}>
      <span
        className="flex flex-wrap items-center gap-0.5 rounded-md border border-line bg-sunken p-0.5"
        role="group"
      >
        {options.map((item) => (
          <Tooltip key={item.key} content={item.title}>
            <button
              type="button"
              aria-pressed={item.key === value}
              className={cn(
                'cursor-pointer rounded-sm px-2 py-0.5 text-xs transition-colors duration-[var(--dur-1)]',
                item.key === value
                  ? 'bg-accent font-medium text-on-accent shadow-card'
                  : 'text-muted hover:bg-hover hover:text-fg',
              )}
              onClick={() => onPick(item.key)}
            >
              {item.label}
            </button>
          </Tooltip>
        ))}
      </span>
    </Hint>
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
