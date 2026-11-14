// 各软件积分消耗：环形占比图 + 明细表 + 选中后的单软件按天收支。
//
// 口径与总览完全一致（账号日余额增量），这里只做「先分组再汇总」；
// 数字全部由 points 现算，自定义区间裁掉区间外的点之后，环形图与表格才不会各说各话。
// 环形扇区与表格行指向同一个选中态：点哪边都联动，另一边高亮提示——
// 颜色沿用同一份 PALETTE 且按表格行序分配，看中扇区下一眼就能在表里对上名字。
import { useMemo, useState } from 'react';
import type { CreditFlowPoint, CreditPlatformFlow } from '@/lib/types';
import { fmtAmount, fmtDelta } from '@/lib/format';
import { cn } from '@/components/ui/button';
import type { ChartOption, TopLevelFormatterParams } from '@/lib/echarts';
import { CreditFlowChart } from './credit-flow';
import { EChart, useChartPalette } from './echart';

interface Row extends CreditPlatformFlow {
  earned: number;
  spent: number;
  net: number;
  balance: number;
}

/** 区间合计：与总览共用一套「进 / 出 / 期末余额」口径。 */
function summarize(points: CreditFlowPoint[]) {
  let earned = 0;
  let spent = 0;
  for (const point of points) {
    earned += point.earned;
    spent += point.spent;
  }
  return {
    earned,
    spent,
    net: earned - spent,
    balance: points.length > 0 ? points[points.length - 1].balance : 0,
  };
}

export function CreditPlatformSplit({
  platforms,
  note,
}: {
  platforms: CreditPlatformFlow[];
  /** 区间说明（最近 N 天 / 自定义起止），由页面统一口径 */
  note: string;
}) {
  const palette = useChartPalette();
  const [selected, setSelected] = useState<string | null>(null);
  // 环形（canvas）与表格色点（HTML）同序同色。canvas 吃不到 CSS 变量，前三个
  // 取色板实值（useChartPalette 已按主题解析），后三个是主题无关的固定色。
  const SERIES_COLORS = [palette.accent, palette.ok, palette.warn, '#8b5cf6', '#06b6d4', '#f472b6'];
  const rows = useMemo<Row[]>(
    () =>
      platforms
        .map((row) => ({ ...row, ...summarize(row.points) }))
        .sort((a, b) => b.spent - a.spent || b.earned - a.earned),
    [platforms],
  );
  const totalSpent = rows.reduce((sum, row) => sum + row.spent, 0);
  const totalEarned = rows.reduce((sum, row) => sum + row.earned, 0);
  const active = rows.find((row) => row.platform === selected) ?? null;
  const donutRows = rows.filter((row) => row.spent > 0);

  const toggle = (row: Row) => {
    if (row.points.length === 0) return;
    setSelected((current) => (current === row.platform ? null : row.platform));
  };

  // 环形与表格同色同序：颜色按表格行序分配，扇区点击回到同一个选中态
  const colorFor = (row: Row) => SERIES_COLORS[rows.indexOf(row) % SERIES_COLORS.length];

  const donutOption = useMemo<ChartOption | null>(() => {
    if (donutRows.length === 0) return null;
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
          const row = rows.find((item) => item.label === first?.name);
          if (!row) return '';
          const share = totalSpent === 0 ? 0 : (row.spent / totalSpent) * 100;
          const line = (label: string, value: string, color?: string) =>
            `<div style="display:flex;justify-content:space-between;gap:18px;margin-top:3px">` +
            `<span style="color:${palette.muted}">${label}</span>` +
            `<span style="font-variant-numeric:tabular-nums;${color ? `color:${color};font-weight:600` : ''}">${value}</span></div>`;
          return (
            `<div style="font-weight:600;margin-bottom:2px">${row.label}</div>` +
            line('消耗', `-${fmtAmount(row.spent)}`, palette.bad) +
            line('到账', `+${fmtAmount(row.earned)}`, palette.ok) +
            line('消耗占比', `${share.toFixed(1)}%`)
          );
        },
      },
      series: [
        {
          name: '各软件消耗',
          type: 'pie',
          radius: ['58%', '82%'],
          center: ['50%', '50%'],
          minAngle: 2,
          selectedMode: false,
          label: { show: false },
          itemStyle: { borderColor: palette.card, borderWidth: 2 },
          emphasis: {
            scaleSize: 5,
            itemStyle: { shadowBlur: 14, shadowOffsetY: 2, shadowColor: 'rgba(0,0,0,.28)' },
          },
          data: donutRows.map((row) => ({
            name: row.label,
            value: row.spent,
            itemStyle: {
              color: colorFor(row),
              // 有选中时压暗其余扇区：联动感比高亮选中本身更直观
              opacity: active && active.platform !== row.platform ? 0.35 : 1,
            },
          })),
        },
      ],
    };
    // colorFor 依赖 rows 的行序，一并进依赖
  }, [donutRows, rows, totalSpent, active, palette]);

  if (rows.length === 0) {
    return (
      <section className="rounded-lg border border-line bg-card p-3">
        <div className="mb-2 flex flex-wrap items-baseline gap-2">
          <h3 className="text-xs font-semibold tracking-wide text-fg2">各软件消耗</h3>
          <span className="flex-1" />
          <span className="text-xs text-muted">{note}</span>
        </div>
        <div className="rounded-md border border-dashed border-line px-3 py-6 text-center text-xs text-muted">
          还没有按软件的收支数据：每个软件至少要有两天的余额快照，才能从差值看出进账与消耗
        </div>
      </section>
    );
  }

  return (
    <section className="rounded-lg border border-line bg-card p-3">
      <div className="mb-2 flex flex-wrap items-baseline gap-2">
        <h3 className="text-xs font-semibold tracking-wide text-fg2">各软件消耗</h3>
        <span className="flex-1" />
        <span className="text-xs text-muted">{note}</span>
      </div>

      <div className="grid gap-3 lg:grid-cols-[240px_minmax(0,1fr)]">
        <div>
          {donutOption ? (
            <div className="relative">
              <EChart
                option={donutOption}
                height={200}
                ariaLabel="各软件消耗占比"
                className="w-full"
                onClick={(name) => {
                  const row = rows.find((item) => item.label === name);
                  if (row) toggle(row);
                }}
              />
              {/* 中心数字用 HTML 叠加：省注册 Title/Graphic 组件，位置与环心天然对齐 */}
              <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
                <span className="text-[10px] text-muted">合计消耗</span>
                <span className="text-md font-semibold tabular-nums">{fmtAmount(totalSpent)}</span>
              </div>
            </div>
          ) : (
            <div className="flex h-[200px] items-center justify-center rounded-md border border-dashed border-line px-3 text-center text-xs text-muted">
              区间内没有消耗记录
            </div>
          )}
        </div>

        <div>
          <div className="mb-1 flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-xs text-muted">
            <span>
              合计到账 <span className="font-medium text-ok">+{fmtAmount(totalEarned)}</span>
            </span>
            <span className="flex-1" />
            <span>点环形扇区或表格行，看该软件的按天收支</span>
          </div>

          <table className="w-full border-collapse text-xs">
            <thead>
              <tr className="text-left text-muted">
                <th className="py-1 font-medium">软件</th>
                <th className="py-1 text-right font-medium">消耗</th>
                <th className="py-1 text-right font-medium">到账</th>
                <th className="py-1 text-right font-medium">净额</th>
                <th className="py-1 text-right font-medium">期末余额</th>
                <th className="w-32 py-1 pl-3 font-medium">消耗占比</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => {
                const share = totalSpent === 0 ? 0 : (row.spent / totalSpent) * 100;
                const current = active?.platform === row.platform;
                return (
                  <tr
                    key={row.platform}
                    tabIndex={row.points.length > 0 ? 0 : undefined}
                    title={row.points.length > 0 ? '点开看这个软件每天进出了多少' : undefined}
                    onClick={() => toggle(row)}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault();
                        toggle(row);
                      }
                    }}
                    className={cn(
                      'border-t border-line/60',
                      row.points.length > 0 && 'cursor-pointer hover:bg-hover',
                      current && 'bg-hover',
                    )}
                  >
                    <td className="py-1 whitespace-nowrap">
                      <span className="flex min-w-0 items-center gap-1.5">
                        <span
                          className="inline-block h-2 w-2 shrink-0 rounded-sm"
                          style={{
                            background: colorFor(row),
                            opacity: active && !current ? 0.35 : 1,
                          }}
                        />
                        <span className="truncate">{row.label}</span>
                        <span className="shrink-0 text-muted">{row.accounts} 个账号</span>
                      </span>
                    </td>
                    <td
                      className={cn(
                        'py-1 text-right tabular-nums',
                        row.spent > 0 && 'text-bad',
                      )}
                    >
                      {row.spent > 0 ? `-${fmtAmount(row.spent)}` : '—'}
                    </td>
                    <td
                      className={cn(
                        'py-1 text-right tabular-nums',
                        row.earned > 0 && 'text-ok',
                      )}
                    >
                      {row.earned > 0 ? `+${fmtAmount(row.earned)}` : '—'}
                    </td>
                    <td
                      className={cn(
                        'py-1 text-right tabular-nums',
                        row.net > 0 && 'text-ok',
                        row.net < 0 && 'text-bad',
                      )}
                    >
                      {fmtDelta(row.net)}
                    </td>
                    <td className="py-1 text-right tabular-nums">{fmtAmount(row.balance)}</td>
                    <td className="py-1 pl-3">
                      <div className="flex items-center gap-1.5">
                        <div className="h-1.5 flex-1 rounded-full bg-hover">
                          <div
                            className="h-1.5 rounded-full bg-accent"
                            style={{ width: share > 0 ? `${Math.max(2, share)}%` : '0%' }}
                          />
                        </div>
                        <span className="w-10 text-right tabular-nums text-muted">
                          {share.toFixed(0)}%
                        </span>
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>

      {active && (
        <div className="mt-3 rounded-md border border-line/70 p-2">
          <div className="mb-1 flex flex-wrap items-baseline gap-2 text-xs">
            <span className="font-semibold text-fg2">{active.label} · 按天收支</span>
            <span className="text-muted">{active.accounts} 个账号</span>
            <span className="flex-1" />
            <button
              className="cursor-pointer text-muted hover:text-fg"
              onClick={() => setSelected(null)}
            >
              收起
            </button>
          </div>
          <CreditFlowChart points={active.points} />
        </div>
      )}
    </section>
  );
}
