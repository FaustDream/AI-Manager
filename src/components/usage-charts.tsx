// 用量图表：按天分平台的折线 + 模型占比环形（ECharts 按需注册）。
//
// 与积分收支不是同一份账：这份来自客户端落盘文件（每 30 分钟增量扫描），
// 能归因到具体软件与模型；积分收支来自账号余额快照，回答的是「进出多少」。
//
// 为什么是折线：这张卡回答的是「走势」，相邻两点的斜率比单根柱子的高度更直观。
// 平台各一条线（颜色与图例一一对应），图例可以点选隐藏平台——只想看某家时不用再靠肉眼滤线；
// 纵轴上限仍按「每日合计」取整，这样几条线之间的相对关系不会被压扁。
import { useMemo } from 'react';
import type { UsageDay, UsageModel } from '@/lib/types';
import { fmtTokens, fmtTokensExact, fmtTokensShort } from '@/lib/format';
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
    // canvas 吃不到 CSS 变量：前三个取色板实值（随主题解析），后三个主题无关
    const colors = [palette.accent, palette.ok, palette.warn, '#8b5cf6', '#06b6d4', '#f472b6'];
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
        （{chart.peak.date.slice(5)}）· 点图例可隐藏对应平台
      </div>
      <EChart
        option={option}
        height={216}
        ariaLabel={`Token 消耗趋势：${days[0].date.slice(5)} 至 ${days[days.length - 1].date.slice(5)}`}
        className="w-full"
      />
    </div>
  );
}

/** 环形与图例列表同序同色；前三个取主题令牌实值，后三个主题无关。 */
function seriesColors(palette: { accent: string; ok: string; warn: string }): string[] {
  return [palette.accent, palette.ok, palette.warn, '#8b5cf6', '#06b6d4', '#f472b6'];
}

export function ModelDonut({ models }: { models: UsageModel[] }) {
  const palette = useChartPalette();
  const colors = seriesColors(palette);

  const slices = useMemo(() => {
    const total = models.reduce((sum, model) => sum + model.tokens, 0);
    if (total === 0) return null;
    const top = models.slice(0, 5);
    const rest = models.slice(5);
    const restTokens = rest.reduce((sum, model) => sum + model.tokens, 0);
    const items = [...top];
    if (restTokens > 0) {
      items.push({ model: '其他', tokens: restTokens, credit: 0, calls: 0 });
    }
    return { items, total };
  }, [models]);

  const option = useMemo<ChartOption | null>(() => {
    if (!slices) return null;
    return {
      animationDuration: 350,
      tooltip: {
        trigger: 'item',
        backgroundColor: palette.card,
        borderColor: palette.line,
        textStyle: { color: palette.fg, fontSize: 12 },
        extraCssText: 'border-radius:6px;box-shadow:0 6px 20px rgba(0,0,0,.18);',
        formatter: (params: TopLevelFormatterParams) => {
          const first = Array.isArray(params) ? params[0] : params;
          const value = Number(first?.value ?? 0);
          const percent = ((value / slices.total) * 100).toFixed(1);
          return (
            `<div style="font-weight:600;margin-bottom:2px">${first?.name ?? ''}</div>` +
            `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:3px">` +
            `<span style="color:${palette.muted}">Token</span>` +
            `<span style="font-variant-numeric:tabular-nums;font-weight:600">${fmtTokensExact(value)}</span></div>` +
            `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:3px">` +
            `<span style="color:${palette.muted}">占比</span>` +
            `<span style="font-variant-numeric:tabular-nums">${percent}%</span></div>`
          );
        },
      },
      series: [
        {
          name: '模型占比',
          type: 'pie',
          radius: ['56%', '80%'],
          center: ['50%', '50%'],
          minAngle: 2,
          selectedMode: false,
          label: { show: false },
          labelLine: { show: false },
          itemStyle: { borderColor: palette.card, borderWidth: 2 },
          emphasis: {
            scaleSize: 5,
            itemStyle: { shadowBlur: 14, shadowOffsetY: 2, shadowColor: 'rgba(0,0,0,.28)' },
          },
          data: slices.items.map((model, index) => ({
            name: model.model,
            value: model.tokens,
            itemStyle: { color: colors[index % colors.length] },
          })),
        },
      ],
    };
  }, [slices, palette, colors]);

  if (!slices || !option) return null;

  return (
    <div className="flex items-center gap-3">
      <div className="relative shrink-0">
        <EChart option={option} height={160} ariaLabel="模型占比" className="w-[160px]" />
        {/* 中心数字用 HTML 叠加：省注册 Title/Graphic 组件 */}
        <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
          <span className="text-[10px] text-muted">总 Token</span>
          <span className="text-md font-semibold tabular-nums">{fmtTokens(slices.total)}</span>
        </div>
      </div>
      <div className="flex min-w-0 flex-col gap-0.5 text-xs">
        {slices.items.map((model, index) => {
          const percent = (model.tokens / slices.total) * 100;
          return (
            <div key={model.model} className="flex items-center gap-1.5">
              <span
                className="inline-block h-2 w-2 shrink-0 rounded-sm"
                style={{ background: colors[index % colors.length] }}
              />
              <span className="truncate" title={model.model}>
                {model.model}
              </span>
              <span className="ml-auto shrink-0 tabular-nums text-muted">{percent.toFixed(1)}%</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
