// 积分趋势：纯 SVG 折线，不引图表库。
// 数据来自每日余额快照（每次刷新积分时顺手记一条），没有快照的日子不画点，
// 横轴按真实日期铺开——按序号均匀铺会把「中间空了一周」画成「一直很平稳」。
//
// 画布按仪表盘整行宽度设计（960 宽），SVG 只给 `w-full`、高度由比例决定：
// 固定 `h-[180px]` + `w-full` 会让 640 宽的画布在宽屏里居中留白，两侧空一大片。
import { useMemo } from 'react';
import type { TrendPoint } from '@/lib/types';
import { fmtAmount } from '@/lib/format';

const W = 960;
const H = 200;
const PAD_X = 56;
const PAD_TOP = 20;
const PAD_BOTTOM = 26; // 底部日期

/** `YYYY-MM-DD` → UTC 毫秒。只用日期，不做时区换算。 */
function dayMs(date: string): number {
  const [y, m, d] = date.split('-').map(Number);
  return Date.UTC(y, (m || 1) - 1, d || 1);
}

export function CreditTrend({ points }: { points: TrendPoint[] }) {
  const chart = useMemo(() => {
    if (points.length === 0) return null;
    const xs = points.map((p) => dayMs(p.date));
    const ys = points.map((p) => p.balance);
    const x0 = Math.min(...xs);
    const x1 = Math.max(...xs);
    const yMin = Math.min(...ys);
    const yMax = Math.max(...ys);
    // 全平的时候给一个假区间，否则所有点挤在同一条线上看不出是「稳定」还是「没数据」
    const span = yMax - yMin || Math.max(1, yMax * 0.1);
    const innerW = W - PAD_X * 2;
    const innerH = H - PAD_TOP - PAD_BOTTOM;
    const bottom = PAD_TOP + innerH;
    const px = (t: number) =>
      x1 === x0 ? PAD_X + innerW / 2 : PAD_X + ((t - x0) / (x1 - x0)) * innerW;
    const py = (v: number) =>
      PAD_TOP + innerH - ((v - (yMin - span * 0.1)) / (span * 1.2)) * innerH;
    const coords = points.map((_, i) => [px(xs[i]), py(ys[i])] as const);
    const line = coords
      .map(([x, y], i) => `${i === 0 ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)}`)
      .join(' ');
    const area = `${line} L${coords[coords.length - 1][0].toFixed(1)},${bottom} L${coords[0][0].toFixed(1)},${bottom} Z`;
    // 日期刻度：首、中、尾三点，够定位又不挤
    const ticks = [0, Math.floor((points.length - 1) / 2), points.length - 1]
      .filter((value, index, list) => list.indexOf(value) === index)
      .map((index) => ({ x: coords[index][0], date: points[index].date }));
    return {
      coords,
      line,
      area,
      yMin,
      yMax,
      bottom,
      ticks,
      first: points[0],
      last: points[points.length - 1],
    };
  }, [points]);

  if (!chart) {
    return (
      <div className="flex h-[200px] items-center justify-center rounded-md border border-dashed border-line text-xs text-muted">
        还没有余额快照：每次刷新积分会记一条，第二天开始就能看到趋势
      </div>
    );
  }

  const delta = chart.last.balance - chart.first.balance;
  return (
    <div>
      <div className="mb-1 flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-xs">
        <span className="text-muted">最新</span>
        <span className="text-md font-semibold tabular-nums">{fmtAmount(chart.last.balance)}</span>
        <span className={delta > 0 ? 'text-ok' : delta < 0 ? 'text-bad' : 'text-muted'}>
          {delta > 0 ? '+' : ''}
          {fmtAmount(delta)}
        </span>
        <span className="flex-1" />
        <span className="text-muted tabular-nums">
          {chart.first.date} → {chart.last.date}
        </span>
      </div>
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full" role="img" aria-label="积分趋势">
        <defs>
          <linearGradient id="credit-area" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="var(--color-accent)" stopOpacity="0.22" />
            <stop offset="100%" stopColor="var(--color-accent)" stopOpacity="0" />
          </linearGradient>
        </defs>
        {/* 三条横向参考线：顶/中/底，够读出量级即可 */}
        {[0, 0.5, 1].map((ratio) => {
          const y = PAD_TOP + (chart.bottom - PAD_TOP) * ratio;
          return (
            <line
              key={ratio}
              x1={PAD_X}
              x2={W - PAD_X}
              y1={y}
              y2={y}
              stroke="var(--color-line)"
              strokeDasharray="3 4"
            />
          );
        })}
        <text x={4} y={PAD_TOP + 4} className="fill-current text-[10px] text-muted">
          {fmtAmount(chart.yMax)}
        </text>
        <text x={4} y={chart.bottom} className="fill-current text-[10px] text-muted">
          {fmtAmount(chart.yMin)}
        </text>
        <path d={chart.area} fill="url(#credit-area)" />
        <path
          d={chart.line}
          fill="none"
          stroke="var(--color-accent)"
          strokeWidth="2"
          strokeLinejoin="round"
          strokeLinecap="round"
        />
        {chart.coords.length <= 60 &&
          chart.coords.map(([x, y], index) => (
            <circle key={index} cx={x} cy={y} r="2.4" fill="var(--color-accent)">
              <title>{`${points[index].date} · ${fmtAmount(points[index].balance)}`}</title>
            </circle>
          ))}
        {chart.ticks.map((tick, index) => (
          <text
            key={tick.date}
            x={tick.x}
            y={H - 6}
            textAnchor={index === 0 ? 'start' : index === chart.ticks.length - 1 ? 'end' : 'middle'}
            className="fill-current text-[10px] text-muted"
          >
            {tick.date.slice(5)}
          </text>
        ))}
      </svg>
    </div>
  );
}
