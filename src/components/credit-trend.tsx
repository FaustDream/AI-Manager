// 积分趋势：余额快照折线（ECharts 按需注册）。
//
// 数据来自每日余额快照（每次刷新积分时顺手记一条），没有快照的日子不画点，
// 横轴用时间轴按真实日期铺开——按序号均匀铺会把「中间空了一周」画成「一直很平稳」。
// 纵轴 scale 不压零：余额在 8500~9200 之间波动时，压到 0 起点会把波动画成一条直线。
// 最高/最低两点用 markPoint 标出来，「哪天到顶」一眼可见。
import { useMemo } from 'react';
import type { TrendPoint } from '@/lib/types';
import { fmtAmount } from '@/lib/format';
import type { CallbackDataParams, ChartOption, TopLevelFormatterParams } from '@/lib/echarts';
import { EChart, useChartPalette } from './echart';

/** 点数超过这个数才露出底部缩放条：与收支图同规则。 */
const ZOOM_MIN_POINTS = 14;

/** `YYYY-MM-DD` → UTC 毫秒。只用日期，不做时区换算。 */
function dayMs(date: string): number {
  const [y, m, d] = date.split('-').map(Number);
  return Date.UTC(y, (m || 1) - 1, d || 1);
}

/** 时间轴刻度与悬浮都只要月-日；趋势最长 60 天，跨年由收支图的悬浮补全年份。 */
function shortDay(ms: number): string {
  const date = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}`;
}

export function CreditTrend({ points }: { points: TrendPoint[] }) {
  const palette = useChartPalette();

  const option = useMemo<ChartOption | null>(() => {
    if (points.length === 0) return null;
    const hasZoom = points.length > ZOOM_MIN_POINTS;

    return {
      animationDuration: 350,
      grid: { left: 8, right: 14, top: 26, bottom: hasZoom ? 30 : 12, containLabel: true },
      tooltip: {
        trigger: 'axis',
        axisPointer: { type: 'line', lineStyle: { color: palette.line, type: [3, 4] } },
        backgroundColor: palette.card,
        borderColor: palette.line,
        textStyle: { color: palette.fg, fontSize: 12 },
        extraCssText: 'border-radius:6px;box-shadow:0 6px 20px rgba(0,0,0,.18);',
        formatter: (params: TopLevelFormatterParams) => {
          const first = Array.isArray(params) ? params[0] : params;
          const index = first.dataIndex;
          const point = points[index];
          if (!point) return '';
          const prev = index > 0 ? points[index - 1] : null;
          const delta = prev ? point.balance - prev.balance : null;
          return (
            `<div style="font-weight:600;margin-bottom:2px">${point.date}</div>` +
            `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:3px">` +
            `<span style="color:${palette.muted}">余额</span>` +
            `<span style="font-variant-numeric:tabular-nums;font-weight:600">${fmtAmount(point.balance)}</span></div>` +
            (delta !== null
              ? `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:3px">` +
                `<span style="color:${palette.muted}">较前一日</span>` +
                `<span style="font-variant-numeric:tabular-nums;color:${delta >= 0 ? palette.ok : palette.bad}">${fmtAmount(delta)}</span></div>`
              : '')
          );
        },
      },
      xAxis: {
        type: 'time',
        axisLine: { lineStyle: { color: palette.line } },
        axisTick: { show: false },
        axisLabel: {
          color: palette.muted,
          fontSize: 10,
          hideOverlap: true,
          formatter: (value: number) => shortDay(value),
        },
        splitLine: { show: false },
      },
      yAxis: {
        type: 'value',
        scale: true,
        axisLabel: { color: palette.muted, fontSize: 10, formatter: (v: number) => fmtAmount(v) },
        splitLine: { lineStyle: { type: [3, 4], color: palette.line } },
      },
      series: [
        {
          name: '余额',
          type: 'line',
          data: points.map((p) => [dayMs(p.date), p.balance]),
          showSymbol: points.length <= 60,
          symbol: 'circle',
          symbolSize: 5,
          lineStyle: { color: palette.accent, width: 2 },
          itemStyle: { color: palette.accent },
          areaStyle: {
            color: {
              type: 'linear',
              x: 0,
              y: 0,
              x2: 0,
              y2: 1,
              colorStops: [
                { offset: 0, color: `${palette.accent}38` },
                { offset: 1, color: `${palette.accent}00` },
              ],
            },
          },
          emphasis: { scale: 1.6 },
          markPoint: {
            symbol: 'circle',
            symbolSize: 8,
            itemStyle: { color: palette.accentStrong },
            label: {
              show: true,
              position: 'top',
              color: palette.fg,
              fontSize: 10,
              fontWeight: 600,
              formatter: (params: CallbackDataParams) => {
                const v = params.value;
                const amount = typeof v === 'number' ? v : Array.isArray(v) ? v[1] : 0;
                return fmtAmount(Number(amount));
              },
            },
            data: [
              { type: 'max', name: '最高' },
              { type: 'min', name: '最低' },
            ],
          },
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
        还没有余额快照：每次刷新积分会记一条，第二天开始就能看到趋势
      </div>
    );
  }

  const first = points[0];
  const last = points[points.length - 1];
  const delta = last.balance - first.balance;

  return (
    <div>
      <div className="mb-1 flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-xs">
        <span className="text-muted">最新</span>
        <span className="text-md font-semibold tabular-nums">{fmtAmount(last.balance)}</span>
        <span className={delta > 0 ? 'text-ok' : delta < 0 ? 'text-bad' : 'text-muted'}>
          {delta > 0 ? '+' : ''}
          {fmtAmount(delta)}
        </span>
        <span className="flex-1" />
        <span className="text-muted tabular-nums">
          {first.date} → {last.date}
        </span>
      </div>
      <EChart option={option} height={200} ariaLabel="积分趋势" className="w-full" />
    </div>
  );
}
