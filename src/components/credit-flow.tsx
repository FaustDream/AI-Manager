// 积分收支：以 0 为轴的双向柱（ECharts 按需注册，echarts 从 @/lib/echarts 进依赖图）。
//
// 数据来自「按账号的日余额增量」——平台只给余额、不给消费流水，
// 所以这里能回答「今天进账多少、花掉多少」，但不能归因到「花在哪个模型」。
//
// 0 轴按「到账 : 消耗」的实际比例落位（限制在 35%~72%），两边都留出足够高度：
// 手绘版靠手算比例，这里反着做——按 clamp 后的占比直接指定 yAxis 的 min/max，
// 效果一致且交给 ECharts 布局。悬浮提示给出进/出/净/期末四项，缩放条可拖拽细看。
import { useMemo } from 'react';
import type { CreditFlowPoint } from '@/lib/types';
import { fmtAmount, fmtDelta } from '@/lib/format';
import type { ChartOption, TopLevelFormatterParams } from '@/lib/echarts';
import { EChart, useChartPalette } from './echart';

/** 点数超过这个数才露出底部缩放条：7 天一眼看全，多一条拖动条反而多余。 */
const ZOOM_MIN_POINTS = 14;

/** 悬浮里的色点：与柱色同源，看得出哪行是进哪行是出。 */
function marker(color: string): string {
  return `<span style="display:inline-block;width:8px;height:8px;border-radius:2px;background:${color};margin-right:6px"></span>`;
}

export function CreditFlowChart({ points }: { points: CreditFlowPoint[] }) {
  const palette = useChartPalette();

  const option = useMemo<ChartOption | null>(() => {
    if (points.length === 0) return null;
    const maxEarned = Math.max(0, ...points.map((p) => p.earned));
    const maxSpent = Math.max(0, ...points.map((p) => p.spent));
    if (maxEarned + maxSpent === 0) return null;
    const ratio = maxEarned / (maxEarned + maxSpent);
    // 占比限制在 35%~72%：单边独大时对侧仍留出可读空间（口径与手绘版一致）
    const clamped = Math.min(0.72, Math.max(0.35, ratio));
    // 由占比反推轴范围：正空间占比 = yMax / (yMax + yMin)
    const yMax = ratio < clamped ? (maxSpent * clamped) / (1 - clamped) : maxEarned;
    const yMinAbs = ratio > clamped ? (maxEarned * (1 - clamped)) / clamped : maxSpent;
    const hasZoom = points.length > ZOOM_MIN_POINTS;

    return {
      animationDuration: 350,
      grid: { left: 8, right: 8, top: 18, bottom: hasZoom ? 30 : 12, containLabel: true },
      tooltip: {
        trigger: 'axis',
        axisPointer: { type: 'shadow' },
        backgroundColor: palette.card,
        borderColor: palette.line,
        textStyle: { color: palette.fg, fontSize: 12 },
        extraCssText: 'border-radius:6px;box-shadow:0 6px 20px rgba(0,0,0,.18);',
        formatter: (params: TopLevelFormatterParams) => {
          const first = Array.isArray(params) ? params[0] : params;
          const point = points[first.dataIndex];
          if (!point) return '';
          const row = (label: string, value: string, color?: string) =>
            `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:3px">` +
            `<span style="color:${palette.muted}">${label}</span>` +
            `<span style="font-variant-numeric:tabular-nums;${color ? `color:${color};font-weight:600` : ''}">${value}</span></div>`;
          return (
            `<div style="font-weight:600;margin-bottom:2px">${point.date}</div>` +
            row(marker(palette.ok) + '到账', `+${fmtAmount(point.earned)}`, palette.ok) +
            row(marker(palette.bad) + '消耗', `-${fmtAmount(point.spent)}`, palette.bad) +
            row('净额', fmtDelta(point.earned - point.spent)) +
            row('期末余额', fmtAmount(point.balance))
          );
        },
      },
      xAxis: {
        type: 'category',
        data: points.map((p) => p.date),
        axisLine: { lineStyle: { color: palette.line } },
        axisTick: { show: false },
        axisLabel: {
          color: palette.muted,
          fontSize: 10,
          formatter: (value: string) => value.slice(5),
        },
      },
      yAxis: {
        type: 'value',
        min: -yMinAbs,
        max: yMax,
        axisLabel: {
          color: palette.muted,
          fontSize: 10,
          formatter: (value: number) =>
            value === 0 ? '0' : `${value > 0 ? '+' : '-'}${fmtAmount(Math.abs(value))}`,
        },
        splitLine: { lineStyle: { type: [3, 4], color: palette.line } },
      },
      series: [
        {
          name: '到账',
          type: 'bar',
          stack: 'flow',
          barMaxWidth: 22,
          itemStyle: { color: palette.ok, borderRadius: [3, 3, 0, 0] },
          data: points.map((p) => p.earned),
        },
        {
          name: '消耗',
          type: 'bar',
          stack: 'flow',
          barMaxWidth: 22,
          itemStyle: { color: palette.bad, borderRadius: [0, 0, 3, 3] },
          data: points.map((p) => -p.spent),
        },
      ],
      dataZoom: hasZoom
        ? [
            { type: 'inside' },
            {
              type: 'slider',
              height: 16,
              bottom: 0,
              brushSelect: false,
              borderColor: palette.line,
              fillerColor: `${palette.accent}1f`,
              handleStyle: { color: palette.accent },
              moveHandleStyle: { color: palette.accent },
              dataBackground: {
                lineStyle: { color: palette.line },
                areaStyle: { color: palette.hover },
              },
              selectedDataBackground: {
                lineStyle: { color: palette.accent },
                areaStyle: { color: `${palette.accent}33` },
              },
              textStyle: { color: palette.muted, fontSize: 10 },
            },
          ]
        : [],
    };
  }, [points, palette]);

  if (!option) {
    return (
      <div className="flex h-[200px] items-center justify-center rounded-md border border-dashed border-line text-xs text-muted">
        还没有收支数据：每天刷新积分后会记一条余额，第二天的差值就能算出收支
      </div>
    );
  }

  const totalEarned = points.reduce((sum, p) => sum + p.earned, 0);
  const totalSpent = points.reduce((sum, p) => sum + p.spent, 0);
  // 「期末」= 区间内最后一个有快照的日子，不是今天：没有新快照时余额就是停在那一刻
  const balance = points[points.length - 1].balance;

  return (
    <div>
      <div className="mb-1 flex flex-wrap items-baseline gap-x-4 gap-y-0.5 text-xs">
        <span>
          <span className="text-muted">到账 </span>
          <span className="font-semibold tabular-nums text-ok">+{fmtAmount(totalEarned)}</span>
        </span>
        <span>
          <span className="text-muted">消耗 </span>
          <span className="font-semibold tabular-nums text-bad">-{fmtAmount(totalSpent)}</span>
        </span>
        <span>
          <span className="text-muted">净额 </span>
          <span
            className={`font-semibold tabular-nums ${
              totalEarned - totalSpent >= 0 ? 'text-fg' : 'text-bad'
            }`}
          >
            {totalEarned - totalSpent >= 0 ? '+' : ''}
            {fmtAmount(totalEarned - totalSpent)}
          </span>
        </span>
        <span>
          <span className="text-muted">期末余额 </span>
          <span className="font-semibold tabular-nums">{fmtAmount(balance)}</span>
        </span>
        <span className="flex-1" />
        <span className="text-muted">按日汇总 · 消耗为平台侧扣减</span>
      </div>
      <EChart option={option} height={200} ariaLabel="积分收支" className="w-full" />
    </div>
  );
}
