// 积分收支：以 0 为轴的双向柱。
//
// 数据来自「按账号的日余额增量」——平台只给余额、不给消费流水，
// 所以这里能回答「今天进账多少、花掉多少」，但不能归因到「花在哪个模型」。
//
// 画布按仪表盘整行宽度设计（960 宽），SVG 只给 `w-full`、高度由比例决定：
// 固定 `h-[180px]` + `w-full` 会让 640 宽的画布在宽屏里居中留白，两侧空一大片。
import { useMemo } from 'react';
import type { CreditFlowPoint } from '@/lib/types';
import { fmtAmount } from '@/lib/format';

const W = 960;
const H = 200;
const PAD_X = 52;
const PAD_Y = 20;

export function CreditFlowChart({ points }: { points: CreditFlowPoint[] }) {
  const chart = useMemo(() => {
    if (points.length === 0) return null;
    const maxEarned = Math.max(0, ...points.map((p) => p.earned));
    const maxSpent = Math.max(0, ...points.map((p) => p.spent));
    const innerW = W - PAD_X * 2;
    const innerH = H - PAD_Y * 2;
    // 0 轴按「到账 : 消耗」的实际比例落位（限制在 35%~72%），两边都留出足够高度
    const ratio = maxEarned + maxSpent === 0 ? 0.5 : maxEarned / (maxEarned + maxSpent);
    const zeroY = PAD_Y + innerH * Math.min(0.72, Math.max(0.35, ratio));
    const upRoom = zeroY - PAD_Y;
    const downRoom = H - PAD_Y - zeroY;
    // 取两侧都装得下的比例：用 max 会让另一侧溢出画布，柱子和刻度一起被裁
    const scale = Math.min(
      upRoom / Math.max(maxEarned, 1e-9),
      downRoom / Math.max(maxSpent, 1e-9),
    );
    const slot = innerW / points.length;
    const barW = Math.max(2, Math.min(22, slot * 0.55));
    const bars = points.map((point, index) => {
      const x = PAD_X + slot * index + slot / 2;
      const up = point.earned * scale;
      const down = point.spent * scale;
      return { x, point, up, down };
    });
    const totalEarned = points.reduce((sum, p) => sum + p.earned, 0);
    const totalSpent = points.reduce((sum, p) => sum + p.spent, 0);
    // 「期末」= 区间内最后一个有快照的日子，不是今天：没有新快照时余额就是停在那一刻
    const balance = points[points.length - 1].balance;
    // 日期刻度：首、中、尾三个点，够定位又不挤
    const ticks = [0, Math.floor((points.length - 1) / 2), points.length - 1]
      .filter((value, index, list) => list.indexOf(value) === index)
      .map((index) => ({ x: PAD_X + slot * index + slot / 2, date: points[index].date }));
    return {
      bars,
      barW,
      zeroY,
      maxEarned,
      maxSpent,
      scale,
      totalEarned,
      totalSpent,
      balance,
      ticks,
    };
  }, [points]);

  if (!chart) {
    return (
      <div className="flex h-[200px] items-center justify-center rounded-md border border-dashed border-line text-xs text-muted">
        还没有收支数据：每天刷新积分后会记一条余额，第二天的差值就能算出收支
      </div>
    );
  }

  return (
    <div>
      <div className="mb-1 flex flex-wrap items-baseline gap-x-4 gap-y-0.5 text-xs">
        <span>
          <span className="text-muted">到账 </span>
          <span className="font-semibold tabular-nums text-ok">+{fmtAmount(chart.totalEarned)}</span>
        </span>
        <span>
          <span className="text-muted">消耗 </span>
          <span className="font-semibold tabular-nums text-bad">-{fmtAmount(chart.totalSpent)}</span>
        </span>
        <span>
          <span className="text-muted">净额 </span>
          <span
            className={`font-semibold tabular-nums ${
              chart.totalEarned - chart.totalSpent >= 0 ? 'text-fg' : 'text-bad'
            }`}
          >
            {chart.totalEarned - chart.totalSpent >= 0 ? '+' : ''}
            {fmtAmount(chart.totalEarned - chart.totalSpent)}
          </span>
        </span>
        <span>
          <span className="text-muted">期末余额 </span>
          <span className="font-semibold tabular-nums">{fmtAmount(chart.balance)}</span>
        </span>
        <span className="flex-1" />
        <span className="text-muted">按日汇总 · 消耗为平台侧扣减</span>
      </div>
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full" role="img" aria-label="积分收支">
        <line
          x1={PAD_X}
          x2={W - PAD_X}
          y1={chart.zeroY}
          y2={chart.zeroY}
          stroke="var(--color-line)"
        />
        {/* 刻度贴着「实际最大值」而不是画布上下沿：上下留白不等高，贴边会读错量级 */}
        <text
          x={4}
          y={chart.zeroY - chart.maxEarned * chart.scale + 4}
          className="fill-current text-[10px] text-muted"
        >
          +{fmtAmount(chart.maxEarned)}
        </text>
        <text
          x={4}
          y={chart.zeroY + chart.maxSpent * chart.scale - 4}
          className="fill-current text-[10px] text-muted"
        >
          -{fmtAmount(chart.maxSpent)}
        </text>
        {chart.bars.map(({ x, point, up, down }) => (
          <g key={point.date}>
            {up > 0.5 && (
              <rect
                x={x - chart.barW / 2}
                y={chart.zeroY - up}
                width={chart.barW}
                height={up}
                rx="1.5"
                fill="var(--color-ok)"
                opacity="0.85"
              >
                <title>{`${point.date} 到账 +${fmtAmount(point.earned)}`}</title>
              </rect>
            )}
            {down > 0.5 && (
              <rect
                x={x - chart.barW / 2}
                y={chart.zeroY}
                width={chart.barW}
                height={down}
                rx="1.5"
                fill="var(--color-bad)"
                opacity="0.75"
              >
                <title>{`${point.date} 消耗 -${fmtAmount(point.spent)}`}</title>
              </rect>
            )}
          </g>
        ))}
        {chart.ticks.map((tick, index) => (
          <text
            key={tick.date}
            x={tick.x}
            y={H - 3}
            textAnchor={index === 0 ? 'start' : index === chart.ticks.length - 1 ? 'end' : 'middle'}
            className="fill-current text-[10px] text-muted"
          >
            {tick.date}
          </text>
        ))}
      </svg>
    </div>
  );
}
