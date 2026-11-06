// 积分收支：以 0 为轴的双向柱。
//
// 数据来自「按账号的日余额增量」——平台只给余额、不给消费流水，
// 所以这里能回答「今天进账多少、花掉多少」，但不能归因到「花在哪个模型」。
import { useMemo } from 'react';
import type { CreditFlowPoint } from '@/lib/types';
import { fmtAmount } from '@/lib/format';

const W = 640;
const H = 180;
const PAD_X = 44;
const PAD_Y = 18;

export function CreditFlowChart({ points }: { points: CreditFlowPoint[] }) {
  const chart = useMemo(() => {
    if (points.length === 0) return null;
    const maxAbs = Math.max(1, ...points.map((p) => Math.max(p.earned, p.spent)));
    const innerW = W - PAD_X * 2;
    const innerH = H - PAD_Y * 2;
    // 0 轴略低于中线：到账通常比消耗大，给上方留更多空间
    const zeroY = PAD_Y + innerH * 0.58;
    const scale = Math.max(zeroY - PAD_Y, H - PAD_Y - zeroY) / maxAbs;
    const slot = innerW / points.length;
    const barW = Math.max(2, Math.min(18, slot * 0.55));
    const bars = points.map((point, index) => {
      const x = PAD_X + slot * index + slot / 2;
      const up = point.earned * scale;
      const down = point.spent * scale;
      return { x, point, up, down };
    });
    const totalEarned = points.reduce((sum, p) => sum + p.earned, 0);
    const totalSpent = points.reduce((sum, p) => sum + p.spent, 0);
    return { bars, barW, zeroY, maxAbs, totalEarned, totalSpent };
  }, [points]);

  if (!chart) {
    return (
      <div className="flex h-[180px] items-center justify-center rounded-md border border-dashed border-line text-xs text-muted">
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
        <span className="flex-1" />
        <span className="text-muted">按日汇总 · 消耗为平台侧扣减</span>
      </div>
      <svg viewBox={`0 0 ${W} ${H}`} className="h-[180px] w-full" role="img" aria-label="积分收支">
        <line
          x1={PAD_X}
          x2={W - PAD_X}
          y1={chart.zeroY}
          y2={chart.zeroY}
          stroke="var(--color-line)"
        />
        <text x={4} y={PAD_Y + 4} className="fill-current text-[10px] text-muted">
          +{fmtAmount(chart.maxAbs)}
        </text>
        <text x={4} y={H - PAD_Y} className="fill-current text-[10px] text-muted">
          -{fmtAmount(chart.maxAbs)}
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
        <text x={PAD_X} y={H - 2} className="fill-current text-[10px] text-muted">
          {points[0].date}
        </text>
        <text x={W - PAD_X} y={H - 2} textAnchor="end" className="fill-current text-[10px] text-muted">
          {points[points.length - 1].date}
        </text>
      </svg>
    </div>
  );
}
