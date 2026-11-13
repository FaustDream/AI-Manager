// 各软件积分消耗：份额条 + 明细表 + 选中后的单软件按天收支。
//
// 口径与总览完全一致（账号日余额增量），这里只做「先分组再汇总」；
// 数字全部由 points 现算，自定义区间裁掉区间外的点之后，表格与图表才不会各说各话。
import { useMemo, useState } from 'react';
import type { CreditFlowPoint, CreditPlatformFlow } from '@/lib/types';
import { fmtAmount, fmtDelta } from '@/lib/format';
import { cn } from '@/components/ui/button';
import { CreditFlowChart } from './credit-flow';

/** 份额条与表格同序同色：看中条上的一段，下一行就能对上名字。 */
const PALETTE = [
  'var(--color-accent)',
  'var(--color-ok)',
  'var(--color-warn)',
  '#8b5cf6',
  '#06b6d4',
  '#f472b6',
];

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

  const toggle = (row: Row) => {
    if (row.points.length === 0) return;
    setSelected((current) => (current === row.platform ? null : row.platform));
  };

  return (
    <section className="rounded-lg border border-line bg-card p-3">
      <div className="mb-2 flex flex-wrap items-baseline gap-2">
        <h3 className="text-xs font-semibold tracking-wide text-fg2">各软件消耗</h3>
        <span className="flex-1" />
        <span className="text-xs text-muted">{note}</span>
      </div>

      {rows.length === 0 ? (
        <div className="rounded-md border border-dashed border-line px-3 py-6 text-center text-xs text-muted">
          还没有按软件的收支数据：每个软件至少要有两天的余额快照，才能从差值看出进账与消耗
        </div>
      ) : (
        <>
          <div className="mb-1 flex h-2 w-full overflow-hidden rounded-full bg-hover">
            {totalSpent > 0 &&
              rows.map((row, index) =>
                row.spent > 0 ? (
                  <div
                    key={row.platform}
                    style={{
                      width: `${(row.spent / totalSpent) * 100}%`,
                      background: PALETTE[index % PALETTE.length],
                    }}
                    title={`${row.label} 消耗 ${fmtAmount(row.spent)}`}
                  />
                ) : null,
              )}
          </div>
          <div className="mb-2 flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-xs text-muted">
            <span>
              合计消耗 <span className="font-medium text-fg">{fmtAmount(totalSpent)}</span>
            </span>
            <span>
              合计到账 <span className="font-medium text-ok">+{fmtAmount(totalEarned)}</span>
            </span>
            {totalSpent === 0 && <span>区间内没有消耗记录</span>}
            <span className="flex-1" />
            <span>点一行看该软件的按天收支</span>
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
              {rows.map((row, index) => {
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
                          style={{ background: PALETTE[index % PALETTE.length] }}
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
        </>
      )}
    </section>
  );
}
