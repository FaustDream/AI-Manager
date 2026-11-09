// 用量图表：按天分平台的折线 + 模型占比环形。
// 纯 SVG，不引图表库；空数据由页面统一兜底，这里只画有数据的情况。
//
// 为什么是折线：这张卡回答的是「走势」，相邻两点的斜率比单根柱子的高度更直观。
// 按平台各画一条线（颜色与图例一一对应），纵轴上限仍取「每日合计」，
// 这样几条线之间的相对关系不会被压缩到贴地。
import { useMemo, useState } from 'react';
import type { UsageDay, UsageModel } from '@/lib/types';
import { fmtTokens, fmtTokensExact, fmtTokensShort } from '@/lib/format';

const W = 640;
const H = 216;
const PAD_L = 54; // 左侧刻度
const PAD_R = 10;
const PAD_T = 12;
const PAD_B = 24; // 底部日期
const TICKS = 4;

export const APP_LABEL: Record<string, string> = {
  workbuddy: 'WorkBuddy 国内版',
  workbuddy_intl: 'WorkBuddy 国际版',
  codebuddy: 'CodeBuddy',
  zcode: 'ZCode',
};

const PALETTE = [
  'var(--color-accent)',
  'var(--color-ok)',
  'var(--color-warn)',
  '#8b5cf6',
  '#06b6d4',
  '#f472b6',
];

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

function shortDate(date: string): string {
  return date.slice(5);
}

export function TrendLines({ days }: { days: UsageDay[] }) {
  const [hover, setHover] = useState<number | null>(null);

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

    const n = days.length;
    const innerW = W - PAD_L - PAD_R;
    const innerH = H - PAD_T - PAD_B;
    const slot = innerW / n;
    // 纵轴按「每日合计」取整：单平台线之间的高低差仍看得清，合计也不会顶出框
    const max = niceMax(Math.max(...days.map((day) => day.tokens)));
    const y = (value: number) => PAD_T + innerH - (value / max) * innerH;
    const centerX = (index: number) => PAD_L + slot * index + slot / 2;
    const tokensOf = (day: UsageDay, platform: string) =>
      day.by_app.find((app) => app.platform === platform)?.tokens ?? 0;

    const series = platforms.map((platform, index) => ({
      platform,
      color: PALETTE[index % PALETTE.length],
      points: days.map((day, dayIndex) => ({
        x: centerX(dayIndex),
        y: y(tokensOf(day, platform)),
        value: tokensOf(day, platform),
      })),
    }));

    // 日期标签：最多 7 个，最后一天必须有标签，且不和前一个挤在一起
    const labelStep = Math.max(1, Math.ceil(n / 6));
    const labels: number[] = [];
    for (let i = 0; i < n; i += labelStep) labels.push(i);
    const last = labels[labels.length - 1];
    if (last !== n - 1) {
      if (n - 1 - last < labelStep / 2 && labels.length > 1) labels.pop();
      labels.push(n - 1);
    }

    const peak = days.reduce((best, day) => (day.tokens > best.tokens ? day : best), days[0]);
    const total = days.reduce((sum, day) => sum + day.tokens, 0);

    return {
      platforms,
      totals,
      n,
      slot,
      max,
      y,
      centerX,
      series,
      labels,
      innerH,
      peak,
      total,
      tokensOf,
    };
  }, [days]);

  if (!chart) return null;

  const {
    platforms,
    totals,
    n,
    slot,
    max,
    y,
    centerX,
    series,
    labels,
    innerH,
    peak,
    total,
    tokensOf,
  } = chart;
  const activeIndex = hover !== null && hover >= 0 && hover < n ? hover : null;
  const activeDay = activeIndex === null ? null : days[activeIndex];
  const activeRows =
    activeDay === null
      ? []
      : platforms
          .map((platform, index) => ({
            platform,
            color: PALETTE[index % PALETTE.length],
            tokens: tokensOf(activeDay, platform),
          }))
          .filter((row) => row.tokens > 0)
          .sort((a, b) => b.tokens - a.tokens);

  return (
    <div>
      <div className="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
        {platforms.map((platform, index) => (
          <span key={platform} className="flex items-center gap-1 text-muted">
            <span
              className="inline-block h-0.5 w-3.5 rounded-full"
              style={{ background: PALETTE[index % PALETTE.length] }}
            />
            {appLabel(platform)}
            <span className="tabular-nums text-fg2">{fmtTokens(totals.get(platform) ?? 0)}</span>
          </span>
        ))}
        <span className="flex-1" />
        <span className="text-muted">
          日均 {fmtTokens(total / n)} · 峰值 {fmtTokens(peak.tokens)}（{shortDate(peak.date)}）
        </span>
      </div>

      <div className="relative">
        <svg
          viewBox={`0 0 ${W} ${H}`}
          className="block w-full"
          role="img"
          aria-label={`Token 消耗趋势：${shortDate(days[0].date)} 至 ${shortDate(days[n - 1].date)}，日均 ${fmtTokens(total / n)}`}
          onMouseMove={(event) => {
            const rect = event.currentTarget.getBoundingClientRect();
            const x = ((event.clientX - rect.left) / rect.width) * W;
            const index = Math.floor((x - PAD_L) / slot);
            setHover(index >= 0 && index < n ? index : null);
          }}
          onMouseLeave={() => setHover(null)}
        >
          {/* 悬停列底色：先画，折线盖在上面 */}
          {activeIndex !== null && (
            <rect
              x={PAD_L + slot * activeIndex}
              y={PAD_T}
              width={slot}
              height={innerH}
              fill="var(--color-hover)"
              rx="3"
            />
          )}

          {/* 横向刻度 + 数值 */}
          {Array.from({ length: TICKS + 1 }).map((_, tick) => {
            const value = (max / TICKS) * tick;
            const lineY = y(value);
            return (
              <g key={tick}>
                <line
                  x1={PAD_L}
                  x2={W - PAD_R}
                  y1={lineY}
                  y2={lineY}
                  stroke="var(--color-line)"
                  strokeDasharray={tick === 0 ? undefined : '3 4'}
                />
                <text
                  x={PAD_L - 6}
                  y={lineY + 3}
                  textAnchor="end"
                  className="fill-current text-[10px] text-muted"
                >
                  {fmtTokensShort(value)}
                </text>
              </g>
            );
          })}

          {/* 悬停竖线：比整列底色更贴合折线的读法 */}
          {activeIndex !== null && (
            <line
              x1={centerX(activeIndex)}
              x2={centerX(activeIndex)}
              y1={PAD_T}
              y2={PAD_T + innerH}
              stroke="var(--color-line)"
              strokeDasharray="3 4"
            />
          )}

          {/* 折线：每个平台一条，点即当天 */}
          {series.map((line) => (
            <g key={line.platform}>
              {line.points.length > 1 && (
                <polyline
                  points={line.points.map((point) => `${point.x},${point.y}`).join(' ')}
                  fill="none"
                  stroke={line.color}
                  strokeWidth={2}
                  strokeLinejoin="round"
                  strokeLinecap="round"
                />
              )}
              {line.points.map((point, index) => (
                <circle
                  key={index}
                  cx={point.x}
                  cy={point.y}
                  r={activeIndex === index ? 3.4 : 2.2}
                  fill={line.color}
                  opacity={activeIndex === null || activeIndex === index ? 1 : 0.45}
                />
              ))}
            </g>
          ))}

          {/* 日期 */}
          {labels.map((index) => (
            <text
              key={index}
              x={centerX(index)}
              y={H - 8}
              textAnchor="middle"
              className="fill-current text-[10px] text-muted"
            >
              {shortDate(days[index].date)}
            </text>
          ))}
        </svg>

        {activeDay && activeIndex !== null && (
          <div
            className="pointer-events-none absolute top-2 z-10 min-w-[168px] rounded-md border border-line bg-card px-2.5 py-2 text-xs shadow-lg"
            style={{
              left: `${(centerX(activeIndex) / W) * 100}%`,
              transform:
                activeIndex < n * 0.3
                  ? 'translateX(-8px)'
                  : activeIndex > n * 0.7
                    ? 'translateX(calc(-100% + 8px))'
                    : 'translateX(-50%)',
            }}
          >
            <div className="mb-1 flex items-baseline gap-1.5">
              <span className="font-semibold">{activeDay.date}</span>
              <span className="text-muted">周{weekday(activeDay.date)}</span>
            </div>
            {activeRows.length === 0 ? (
              <div className="text-muted">这天没有调用记录</div>
            ) : (
              activeRows.map((row) => (
                <div key={row.platform} className="flex items-center gap-1.5">
                  <span
                    className="inline-block h-2 w-2 shrink-0 rounded-full"
                    style={{ background: row.color }}
                  />
                  <span className="truncate text-muted">{appLabel(row.platform)}</span>
                  <span className="ml-auto tabular-nums">{fmtTokens(row.tokens)}</span>
                </div>
              ))
            )}
            <div className="mt-1 flex items-baseline gap-2 border-t border-line pt-1">
              <span className="text-muted">合计</span>
              <span className="tabular-nums font-semibold">{fmtTokens(activeDay.tokens)}</span>
              <span className="ml-auto text-muted">{activeDay.calls} 次</span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export function ModelDonut({ models }: { models: UsageModel[] }) {
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
    let angle = -Math.PI / 2;
    const r = 62;
    const cx = 80;
    const cy = 80;
    const arcs = items.map((item, index) => {
      const sweep = (item.tokens / total) * Math.PI * 2;
      const x1 = cx + r * Math.cos(angle);
      const y1 = cy + r * Math.sin(angle);
      const x2 = cx + r * Math.cos(angle + sweep);
      const y2 = cy + r * Math.sin(angle + sweep);
      const large = sweep > Math.PI ? 1 : 0;
      const path =
        items.length === 1
          ? `M${cx},${cy - r} A${r},${r} 0 1 1 ${cx - 0.01},${cy - r} Z`
          : `M${cx},${cy} L${x1.toFixed(2)},${y1.toFixed(2)} A${r},${r} 0 ${large} 1 ${x2.toFixed(2)},${y2.toFixed(2)} Z`;
      angle += sweep;
      return { ...item, path, color: PALETTE[index % PALETTE.length], percent: (item.tokens / total) * 100 };
    });
    return { arcs, total };
  }, [models]);

  if (!slices) return null;

  return (
    <div className="flex items-center gap-3">
      <svg viewBox="0 0 160 160" className="h-[150px] w-[150px] shrink-0" role="img" aria-label="模型占比">
        {slices.arcs.map((arc) => (
          <path key={arc.model} d={arc.path} fill={arc.color} opacity="0.8">
            <title>{`${arc.model} · ${fmtTokensExact(arc.tokens)} tokens · ${arc.percent.toFixed(1)}%`}</title>
          </path>
        ))}
        <circle cx="80" cy="80" r="38" fill="var(--color-card)" />
        <text x="80" y="76" textAnchor="middle" className="fill-current text-[10px] text-muted">
          总 Token
        </text>
        <text x="80" y="90" textAnchor="middle" className="fill-current text-[11px] font-semibold">
          {fmtTokens(slices.total)}
        </text>
      </svg>
      <div className="flex min-w-0 flex-col gap-0.5 text-xs">
        {slices.arcs.map((arc) => (
          <div key={arc.model} className="flex items-center gap-1.5">
            <span className="inline-block h-2 w-2 shrink-0 rounded-sm" style={{ background: arc.color }} />
            <span className="truncate" title={arc.model}>
              {arc.model}
            </span>
            <span className="ml-auto shrink-0 tabular-nums text-muted">{arc.percent.toFixed(1)}%</span>
          </div>
        ))}
      </div>
    </div>
  );
}
