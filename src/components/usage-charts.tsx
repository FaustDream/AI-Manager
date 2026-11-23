// 用量图表：Token 消耗趋势折线（ECharts 按需注册）。
//
// 与积分收支不是同一份账：这份来自客户端落盘文件（每 30 分钟增量扫描），
// 能归因到具体软件与模型；积分收支来自账号余额快照，回答的是「进出多少」。
// 各软件与模型的占比用排名列表（环形图只配「一家独大」的份额）。
//
// 为什么是折线：这张卡回答的是「走势」，相邻两点的斜率比单根柱子的高度更直观。
// 平台各一条线（颜色与图例一一对应），图例可以点选隐藏平台——只想看某家时不用再靠肉眼滤线；
// 纵轴上限仍按「每日合计」取整，这样几条线之间的相对关系不会被压扁。
import { useMemo } from 'react';
import type { UsageDay } from '@/lib/types';
import { fmtTokens, fmtTokensShort } from '@/lib/format';
import type { ChartOption, TopLevelFormatterParams } from '@/lib/echarts';
import { EChart, useChartPalette } from './echart';

export const APP_LABEL: Record<string, string> = {
  workbuddy: 'WorkBuddy 国内版',
  workbuddy_intl: 'WorkBuddy 国际版',
  codebuddy: 'CodeBuddy',
  zcode: 'ZCode',
};

export function appLabel(platform: string): string {
  return APP_LABEL[platform] ?? platform;
}

/** 刻度上限取整：7.2 亿 → 8 亿，刻度才落在 0 / 2 / 4 / 6 / 8 这种整数上。 */
function niceMax(value: number): number {
  if (!(value > 0)) return 1;
  const exp = 10 ** Math.floor(Math.log10(value));
  const unit = value / exp;
  const steps = [1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10];
  return (steps.find((step) => unit <= step) ?? 10) * exp;
}

function weekday(date: string): string {
  const day = new Date(`${date}T00:00:00`);
  return Number.isNaN(day.getTime()) ? '' : '日一二三四五六'[day.getDay()];
}

/** 悬浮里的色点：与线色同源。 */
function marker(color: string): string {
  return `<span style="display:inline-block;width:8px;height:8px;border-radius:2px;background:${color};margin-right:6px"></span>`;
}

export function TrendLines({ days }: { days: UsageDay[] }) {
  const palette = useChartPalette();

  const chart = useMemo(() => {
    if (days.length === 0) return null;
    const platforms = [...new Set(days.flatMap((day) => day.by_app.map((app) => app.platform)))];
    // 图例顺序按区间总量降序，颜色也跟着定下来，和折线一一对应
    const totals = new Map<string, number>();
    for (const day of days) {
      for (const app of day.by_app) {
        totals.set(app.platform, (totals.get(app.platform) ?? 0) + app.tokens);
      }
    }
    platforms.sort((a, b) => (totals.get(b) ?? 0) - (totals.get(a) ?? 0));
    const tokensOf = (day: UsageDay, platform: string) =>
      day.by_app.find((app) => app.platform === platform)?.tokens ?? 0;
    const peak = days.reduce((best, day) => (day.tokens > best.tokens ? day : best), days[0]);
    const total = days.reduce((sum, day) => sum + day.tokens, 0);
    return { platforms, totals, tokensOf, peak, total };
  }, [days]);

  const option = useMemo<ChartOption | null>(() => {
    if (!chart) return null;
    // canvas 吃不到 CSS 变量：八支序列色从令牌取实值，随主题一起换
    const colors = palette.series;
    const totalByLabel = new Map(
      chart.platforms.map((platform) => [appLabel(platform), chart.totals.get(platform) ?? 0]),
    );

    return {
      animationDuration: 350,
      grid: { left: 8, right: 10, top: 34, bottom: 4, containLabel: true },
      legend: {
        top: 0,
        left: 0,
        icon: 'roundRect',
        itemWidth: 14,
        itemHeight: 3,
        itemGap: 14,
        textStyle: { color: palette.fg2, fontSize: 11 },
        // 图例带区间总量：选择看哪条线之前，先知道每家占多少
        formatter: (name: string) =>
          `${name} ${fmtTokens(totalByLabel.get(name) ?? 0)}`,
      },
      tooltip: {
        trigger: 'axis',
        axisPointer: { type: 'line', lineStyle: { color: palette.line, type: [3, 4] } },
        backgroundColor: palette.card,
        borderColor: palette.line,
        textStyle: { color: palette.fg, fontSize: 12 },
        extraCssText: 'border-radius:6px;box-shadow:0 6px 20px rgba(0,0,0,.18);',
        formatter: (params: TopLevelFormatterParams) => {
          const list = Array.isArray(params) ? params : [params];
          const index = list[0]?.dataIndex;
          const day = index !== undefined ? days[index] : null;
          if (!day) return '';
          const rows = [...list].sort((a, b) => Number(b.value) - Number(a.value));
          const shown = rows.reduce((sum, item) => sum + Number(item.value), 0);
          return (
            `<div style="font-weight:600;margin-bottom:2px">${day.date} 周${weekday(day.date)}</div>` +
            rows
              .map(
                (item) =>
                  `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:3px">` +
                  `<span>${marker(colors[(item.seriesIndex ?? 0) % colors.length])}${item.seriesName}</span>` +
                  `<span style="font-variant-numeric:tabular-nums;font-weight:600">${fmtTokens(Number(item.value))}</span></div>`,
              )
              .join('') +
            `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:6px;padding-top:4px;border-top:1px solid ${palette.line}">` +
            `<span style="color:${palette.muted}">合计</span>` +
            `<span style="font-variant-numeric:tabular-nums;font-weight:600">${fmtTokens(shown)}</span>` +
            `<span style="color:${palette.muted}">${day.calls} 次</span></div>`
          );
        },
      },
      xAxis: {
        type: 'category',
        boundaryGap: false,
        data: days.map((day) => day.date),
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
        max: niceMax(Math.max(...days.map((day) => day.tokens))),
        axisLabel: {
          color: palette.muted,
          fontSize: 10,
          formatter: (value: number) => fmtTokensShort(value),
        },
        splitLine: { lineStyle: { type: [3, 4], color: palette.line } },
      },
      series: chart.platforms.map((platform, index) => ({
        name: appLabel(platform),
        type: 'line' as const,
        data: days.map((day) => chart.tokensOf(day, platform)),
        showSymbol: false,
        symbol: 'circle',
        symbolSize: 5,
        lineStyle: { color: colors[index % colors.length], width: 2 },
        itemStyle: { color: colors[index % colors.length] },
        emphasis: { scale: 1.6 },
      })),
    };
  }, [chart, days, palette]);

  if (!chart || !option) return null;

  return (
    <div>
      <div className="mb-2 text-xs text-muted">
        日均 {fmtTokens(chart.total / days.length)} · 峰值 {fmtTokens(chart.peak.tokens)}
        （{chart.peak.date.slice(5)}）
      </div>
      <EChart
        option={option}
        // 高度对齐左栏的积分收支图（240 + 上方那行汇总）：两栏的子块边界要落在同一条线上
        height={282}
        ariaLabel={`Token 消耗趋势：${days[0].date.slice(5)} 至 ${days[days.length - 1].date.slice(5)}`}
        className="w-full"
      />
    </div>
  );
}
