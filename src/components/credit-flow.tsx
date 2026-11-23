// 积分收支：以 0 为轴的双向柱（ECharts 按需注册，echarts 从 @/lib/echarts 进依赖图）。
//
// 数据来自「按账号的日余额增量」——平台只给余额、不给消费流水，
// 所以这里能回答「今天进账多少、花掉多少」，但不能归因到「花在哪个模型」。
//
// 两种模式：
// - 传入 platforms（各软件明细）→ 按软件堆叠：每天一根柱拆成各家颜色，图例点选
//   隐藏/显示某家，悬浮给出当日各家进/出与期末余额——这是仪表盘主图的默认形态；
// - 不传（单软件的按天收支等场景）→ 简单的到账/消耗两色。
//
// 0 轴按「到账 : 消耗」的实际比例落位（限制在 35%~72%）：按 clamp 后的占比直接指定
// yAxis 的 min/max。缩放条可拖拽细看（14 天以上才出现）。
import { useMemo } from 'react';
import type { CreditFlowPoint, CreditPlatformFlow } from '@/lib/types';
import { fmtAmount, fmtDelta } from '@/lib/format';
import type { ChartOption, TopLevelFormatterParams } from '@/lib/echarts';
import { EChart, useChartPalette } from './echart';

/** 点数超过这个数才露出底部缩放条：7 天一眼看全，多一条拖动条反而多余。 */
const ZOOM_MIN_POINTS = 14;

/** 悬浮里的色点：与柱色同源。 */
function marker(color: string): string {
  return `<span style="display:inline-block;width:8px;height:8px;border-radius:2px;background:${color};margin-right:6px"></span>`;
}

export function CreditFlowChart({
  points,
  platforms,
}: {
  points: CreditFlowPoint[];
  /** 各软件明细：提供且至少一家有流水时进入「按软件堆叠」模式。 */
  platforms?: CreditPlatformFlow[];
}) {
  const palette = useChartPalette();
  // 直接引用色板快照（主题不变时是同一个数组），别在这里新建数组：
  // 它进了下面 useMemo 的依赖，每次渲染新建会让 option 白白重算一遍。
  const colors = palette.series;

  // 堆叠模式：过滤掉区间内完全没流水的软件（图例上挂着一家全零没意义）
  const stackedPlatforms = useMemo(() => {
    const list = (platforms ?? []).filter((p) => p.points.some((pt) => pt.earned > 0 || pt.spent > 0));
    return list.length > 0 ? list : null;
  }, [platforms]);

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
    const stacked = stackedPlatforms !== null;
    // 悬浮按「软件合计」排序要用到每家的按日映射
    const byDate = (stackedPlatforms ?? []).map((p) => ({
      label: p.label,
      map: new Map(p.points.map((pt) => [pt.date, pt])),
    }));

    const series: ChartOption['series'] = [];
    if (stacked && stackedPlatforms) {
      // 同一家软件：向上=到账（实色），向下=消耗（半透明同色），方向即进/出
      for (const [index, platform] of stackedPlatforms.entries()) {
        const color = colors[index % colors.length];
        const map = byDate[index].map;
        const earned = points.map((pt) => map.get(pt.date)?.earned ?? 0);
        const spent = points.map((pt) => -(map.get(pt.date)?.spent ?? 0));
        series.push({
          name: platform.label,
          type: 'bar',
          stack: 'in',
          barMaxWidth: 22,
          itemStyle: { color, borderRadius: [3, 3, 0, 0] },
          data: earned,
        });
        series.push({
          name: platform.label,
          type: 'bar',
          stack: 'out',
          barMaxWidth: 22,
          itemStyle: { color, opacity: 0.5, borderRadius: [0, 0, 3, 3] },
          data: spent,
        });
      }
    } else {
      series.push({
        name: '到账',
        type: 'bar',
        stack: 'flow',
        barMaxWidth: 22,
        itemStyle: { color: palette.ok, borderRadius: [3, 3, 0, 0] },
        data: points.map((p) => p.earned),
      });
      series.push({
        name: '消耗',
        type: 'bar',
        stack: 'flow',
        barMaxWidth: 22,
        itemStyle: { color: palette.bad, borderRadius: [0, 0, 3, 3] },
        data: points.map((p) => -p.spent),
      });
    }

    return {
      animationDuration: 350,
      grid: {
        left: 8,
        right: 8,
        top: stacked ? 36 : 18,
        bottom: hasZoom ? 30 : 12,
        containLabel: true,
      },
      legend: stacked
        ? {
            top: 0,
            left: 0,
            icon: 'roundRect',
            itemWidth: 10,
            itemHeight: 10,
            itemGap: 12,
            textStyle: { color: palette.fg2, fontSize: 10 },
          }
        : undefined,
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
          const head = `<div style="font-weight:600;margin-bottom:2px">${point.date}</div>`;
          if (!stacked || !stackedPlatforms) {
            return (
              head +
              row(marker(palette.ok) + '到账', `+${fmtAmount(point.earned)}`, palette.ok) +
              row(marker(palette.bad) + '消耗', `-${fmtAmount(point.spent)}`, palette.bad) +
              row('净额', fmtDelta(point.earned - point.spent)) +
              row('期末余额', fmtAmount(point.balance))
            );
          }
          // 堆叠模式：一行一家，进/出并排；按当日总量降序
          const perPlatform = byDate
            .map((entry, index) => ({ ...entry, color: colors[index % colors.length] }))
            .map((entry) => ({ ...entry, pt: entry.map.get(point.date) }))
            .filter((entry) => entry.pt && (entry.pt.earned > 0 || entry.pt.spent > 0))
            .sort((a, b) => (b.pt!.earned - b.pt!.spent) - (a.pt!.earned - a.pt!.spent));
          const body = perPlatform
            .map(
              (entry) =>
                `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:3px">` +
                `<span>${marker(entry.color)}${entry.label}</span>` +
                `<span style="font-variant-numeric:tabular-nums"><span style="color:${palette.ok};font-weight:600">+${fmtAmount(entry.pt!.earned)}</span>` +
                ` <span style="color:${palette.muted}">/</span> ` +
                `<span style="color:${palette.bad};font-weight:600">-${fmtAmount(entry.pt!.spent)}</span></span></div>`,
            )
            .join('');
          return (
            head +
            body +
            row('到账合计', `+${fmtAmount(point.earned)}`, palette.ok) +
            row('消耗合计', `-${fmtAmount(point.spent)}`, palette.bad) +
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
      series,
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
  }, [points, stackedPlatforms, palette, colors]);

  if (!option) {
    return (
      <div className="flex h-[240px] items-center justify-center text-xs text-muted">
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
        {/* 只有「按日汇总」需要解释口径；按软件堆叠时柱色与图例已经自解释，不再配说明文字 */}
        {!stackedPlatforms && <span className="text-muted">按日汇总 · 消耗为平台侧扣减</span>}
      </div>
      <EChart
        option={option}
        height={stackedPlatforms ? 240 : 200}
        ariaLabel="积分收支"
        className="w-full"
      />
    </div>
  );
}
