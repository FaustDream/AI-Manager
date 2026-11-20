// 各软件积分消耗：排名行 + 行底份额条 + 选中后的单软件按天收支。
//
// 口径与总览完全一致（账号日余额增量），这里只做「先分组再汇总」；
// 数字全部由 points 现算，自定义区间裁掉区间外的点之后，行与子图才不会各说各话。
//
// 按 dashboard-design 技能重构：ranked breakdown——名字、右对齐数值列、行底 3px
// 消耗份额条，hover 提亮；点一行 = 选中该软件，下方展开它的按天收支（技能的
// selection = 软色填充 + 细环）。
import { useMemo, useState } from 'react';
import type { CreditFlowPoint, CreditPlatformFlow } from '@/lib/types';
import { fmtAmount, fmtDelta } from '@/lib/format';
import { CreditFlowChart } from './credit-flow';
import { ShareBar, shareText } from '@/capabilities/overview/metric';

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
  flush = false,
}: {
  platforms: CreditPlatformFlow[];
  /** 区间说明（最近 N 天 / 自定义起止），由页面统一口径 */
  note: string;
  /** 嵌在别的卡里时去掉自己的边框与内边距（仪表盘左栏就是这么用的） */
  flush?: boolean;
}) {
  // 自带卡壳 or 只出内容：两种形态共用同一份内部结构，避免出现两套表
  const shell = flush ? 'min-w-0' : 'rounded-lg border border-line bg-card p-3';
  const [selected, setSelected] = useState<string | null>(null);
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
  const maxShareRow = Math.max(...rows.map((row) => row.spent), 0);

  const toggle = (row: Row) => {
    if (row.points.length === 0) return;
    setSelected((current) => (current === row.platform ? null : row.platform));
  };

  if (rows.length === 0) {
    return (
      <section className={shell}>
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
    <section className={shell}>
      <div className="mb-1 flex flex-wrap items-baseline gap-2 px-3">
        <h3 className="text-xs font-semibold tracking-wide text-fg2">各软件消耗</h3>
        <span className="flex-1" />
        <span className="text-xs text-muted">{note}</span>
      </div>

      <div className="flex flex-wrap items-baseline gap-x-4 gap-y-0.5 px-3 text-xs text-muted">
        <span>
          合计到账 <span className="font-medium text-ok tabular-nums">+{fmtAmount(totalEarned)}</span>
        </span>
        {totalSpent > 0 && (
          <span>
            合计消耗{' '}
            <span className="font-medium text-bad tabular-nums">-{fmtAmount(totalSpent)}</span>
          </span>
        )}
      </div>

      {/* 表头与行用同一份内边距（px-3）与列宽：差 8px 就会让整列数字看起来歪着 */}
      <div className="px-3 pb-1 text-xs text-muted">
        <div className="flex items-center gap-3">
          <span className="min-w-0 flex-1">软件</span>
          <span className="w-[68px] text-right">到账</span>
          <span className="w-[68px] text-right">消耗</span>
          <span className="w-[68px] text-right">净额</span>
          <span className="w-20 text-right">期末余额</span>
          <span className="w-10 text-right">占比</span>
        </div>
      </div>

      <div>
        {rows.map((row) => {
          const current = active?.platform === row.platform;
          const share = totalSpent === 0 ? 0 : (row.spent / totalSpent) * 100;
          return (
            <button
              key={row.platform}
              title={`${row.label} · ${row.accounts} 个账号 · 到账 +${fmtAmount(row.earned)} · 消耗 ${
                row.spent > 0 ? `-${fmtAmount(row.spent)}` : '—'
              } · 期末余额 ${fmtAmount(row.balance)}${totalSpent > 0 ? ` · 消耗占比 ${shareText(share)}` : ''}`}
              onClick={() => toggle(row)}
              className={`group relative flex h-12 w-full items-center gap-3 rounded-[10px] px-3 pb-1.5 text-left text-sm ${
                current ? 'bg-accent/[.06] ring-1 ring-inset ring-accent/20' : 'hover:bg-hover/60'
              } ${row.points.length > 0 ? 'cursor-pointer' : 'cursor-default'}`}
            >
              {totalSpent > 0 && row.spent > 0 && (
                <ShareBar
                  tone="bad"
                  pct={maxShareRow === 0 ? 0 : (row.spent / maxShareRow) * 100}
                  className="bg-bad/30"
                />
              )}
              <span className="relative flex min-w-0 flex-1 items-baseline gap-1.5">
                <span className="truncate font-medium text-fg">{row.label}</span>
                <span className="shrink-0 text-xs text-muted">{row.accounts} 个账号</span>
              </span>
              <span className="relative w-[68px] text-right tabular-nums text-ok">
                {row.earned > 0 ? `+${fmtAmount(row.earned)}` : '—'}
              </span>
              <span className="relative w-[68px] text-right tabular-nums text-bad">
                {row.spent > 0 ? `-${fmtAmount(row.spent)}` : '—'}
              </span>
              <span className="relative w-[68px] text-right tabular-nums">{fmtDelta(row.net)}</span>
              <span className="relative w-20 text-right tabular-nums">{fmtAmount(row.balance)}</span>
              <span className="relative w-10 text-right text-xs tabular-nums text-muted">
                {totalSpent > 0 ? shareText(share) : '—'}
              </span>
            </button>
          );
        })}
      </div>

      {active && (
        <div className="mt-2 rounded-md border border-line/70 p-2">
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
