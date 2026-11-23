// 列定义驱动的表格（C6）：表头、行、空态、骨架与截断样式收在一处，
// 页面只回答「每列渲染什么」——加一列 = 加一行定义。
// 账号中心（capabilities/account-center/columns.ts）是这套范式的蓝本，
// 这里把它下沉成可复用的表体；账号中心自己的表带拖拽/列宽调整，仍保留原实现。
import { useMemo, useState } from 'react';
import type { CSSProperties, KeyboardEvent, MouseEvent, ReactNode } from 'react';
import { cn } from './button';
import { SkeletonRows, StateBlock } from './form';
import { nextSort, sortRows, type SortState, type SortValue } from '@/lib/sort';

/** 行被激活的事件：鼠标点击或键盘 Enter/Space。两者都带 ctrl/meta，多选判断通用。 */
export type RowActivateEvent = MouseEvent<HTMLTableRowElement> | KeyboardEvent<HTMLTableRowElement>;

export interface Column<R> {
  key: string;
  header: ReactNode;
  /** 固定宽度：数字按 px，字符串原样（Tailwind 类或百分比） */
  width?: number | string;
  align?: 'right';
  /** 单行截断；title 悬停显示全文 */
  truncate?: boolean;
  /**
   * 允许换行。**默认单行**（`whitespace-nowrap`）。
   *
   * 表格里一个格换行，整行就跟着变高——宽度一窄，一行日志能撑成三行高，
   * 而旁边的列全是单行，视觉上像表格错位。所以默认单行，确实要多行的
   * （例如名称下面挂一行说明）才显式打开。
   */
  wrap?: boolean;
  title?: (row: R) => string | undefined;
  /** 复杂单元格的额外样式 */
  className?: string;
  /**
   * 这一列可排序：给出取值函数即可，表头会变成三态排序按钮（升 → 降 → 取消）。
   * 取值返回 `null/空串` 表示「没有值」，排序时一律沉底，不占第一屏。
   */
  sortValue?: (row: R) => SortValue;
  cell: (row: R) => ReactNode;
}

export interface SideColumn<R> {
  head?: ReactNode;
  width?: number | string;
  cell: (row: R) => ReactNode;
}

export interface DataTableProps<R> {
  columns: Column<R>[];
  rows: R[];
  rowKey: (row: R) => string;
  /** 前置列：复选框 / 拖拽柄 */
  lead?: SideColumn<R>;
  /** 操作列（右对齐） */
  trail?: SideColumn<R>;
  loading?: boolean;
  skeletonRows?: number;
  /** 无数据时的整行内容；传 null 表示不渲染空行（空态在表格外自绘） */
  empty?: ReactNode;
  /**
   * 行点击：带原生事件，页面可据此判断 Ctrl / Cmd 做多选（不传事件就没法区分）。
   * 给了它，行就自动键盘可达（tabIndex + Enter/Space + ↑↓），见 P1-9。
   */
  onRowClick?: (row: R, event: RowActivateEvent) => void;
  /** 当前选中行的 key：只用来出 aria-selected，不参与样式（样式仍走 rowClassName） */
  selectedKey?: string | null;
  /** 行右键：自定义上下文菜单（原生菜单已被 context-menu.ts 全局拦掉） */
  onRowContextMenu?: (row: R, event: MouseEvent<HTMLTableRowElement>) => void;
  rowClassName?: (row: R) => string;
  stickyHead?: boolean;
  /** 紧凑密度（hub 系列）：px-2 py-1 / px-2 py-1.5；默认 px-2.5 */
  dense?: boolean;
  className?: string;
  style?: CSSProperties;
  /** 进入页面时的默认排序；不传就是数据原顺序 */
  defaultSort?: SortState | null;
  /** 受控排序：给了它就由外部决定顺序，表头点击只回调 `onSortChange` */
  sort?: SortState | null;
  onSortChange?: (next: SortState | null) => void;
}

const headCls = (dense?: boolean) =>
  cn(
    // 表头同样单行：表头折行会把整列的基线抬高一截，和表体对不齐
    'whitespace-nowrap border-b border-line-strong text-left text-xs font-semibold text-muted',
    dense ? 'px-2 py-1' : 'px-2.5 py-1.5',
  );

const cellCls = (dense?: boolean) => cn('border-b border-line', dense ? 'px-2 py-1.5' : 'px-2.5 py-1.5');

const widthStyle = (w?: number | string): CSSProperties | undefined =>
  w === undefined ? undefined : { width: typeof w === 'number' ? `${w}px` : w };

export function DataTable<R>({
  columns,
  rows,
  rowKey,
  lead,
  trail,
  loading,
  skeletonRows = 4,
  empty,
  onRowClick,
  selectedKey,
  onRowContextMenu,
  rowClassName,
  stickyHead,
  dense,
  className,
  style,
  defaultSort,
  sort,
  onSortChange,
}: DataTableProps<R>) {
  const total = columns.length + (lead ? 1 : 0) + (trail ? 1 : 0);
  const [innerSort, setInnerSort] = useState<SortState | null>(defaultSort ?? null);
  // 受控时完全听外部的：页面可能还要把「排序」写进 URL/偏好，这里不抢方向盘
  const activeSort = sort === undefined ? innerSort : sort;
  const view = useMemo(
    () =>
      sortRows(rows, activeSort, (row, key) => columns.find((col) => col.key === key)?.sortValue?.(row)),
    [rows, activeSort, columns],
  );
  const toggleSort = (key: string) => {
    const next = nextSort(activeSort, key);
    if (sort === undefined) setInnerSort(next);
    onSortChange?.(next);
  };

  return (
    <table className={cn('w-full border-collapse', className)} style={style}>
      <thead className={stickyHead ? 'sticky top-0 bg-bg' : undefined}>
        <tr>
          {lead && (
            <th className={headCls(dense)} style={widthStyle(lead.width)}>
              {lead.head}
            </th>
          )}
          {columns.map((col) => {
            const dir = activeSort?.key === col.key ? activeSort.dir : null;
            return (
              <th
                key={col.key}
                className={cn(headCls(dense), col.align === 'right' && 'text-right')}
                style={widthStyle(col.width)}
                aria-sort={
                  dir ? (dir === 'asc' ? 'ascending' : 'descending') : col.sortValue ? 'none' : undefined
                }
              >
                {col.sortValue ? (
                  <button
                    type="button"
                    onClick={() => toggleSort(col.key)}
                    className="inline-flex cursor-pointer items-center gap-0.5 hover:text-fg"
                    title="点击排序：升序 → 降序 → 取消"
                  >
                    {col.header}
                    <span
                      aria-hidden
                      className={cn('text-[9px] leading-none', dir ? 'opacity-100' : 'opacity-25')}
                    >
                      {dir === 'desc' ? '▼' : '▲'}
                    </span>
                  </button>
                ) : (
                  col.header
                )}
              </th>
            );
          })}
          {trail && (
            <th className={cn(headCls(dense), 'text-right')} style={widthStyle(trail.width)}>
              {trail.head}
            </th>
          )}
        </tr>
      </thead>
      <tbody>
        {loading && <SkeletonRows rows={skeletonRows} cols={total} />}
        {!loading && view.length === 0 && empty !== null && (
          <tr>
            <td colSpan={total}>
              {empty ?? <StateBlock kind="empty" title="没有记录" />}
            </td>
          </tr>
        )}
        {view.map((row) => {
          const key = rowKey(row);
          return (
            <tr
              key={key}
              // 悬停底色只给可点行：不可点的行亮一下会诱导用户去点，点了没反应更像坏了
              className={cn(
                'transition-colors duration-[var(--dur-1)]',
                onRowClick && 'cursor-pointer hover:bg-hover/60',
                rowClassName?.(row),
              )}
              // 行可点就必须键盘可达（P1-9）：Tab 要能停上来，role/aria-selected
              // 让读屏也能报「第几行、选没选」。
              tabIndex={onRowClick ? 0 : undefined}
              role={onRowClick ? 'row' : undefined}
              aria-selected={
                onRowClick && selectedKey !== undefined ? key === selectedKey : undefined
              }
              onClick={onRowClick ? (event) => onRowClick(row, event) : undefined}
              onKeyDown={
                onRowClick
                  ? (event) => {
                      if (event.key === 'Enter' || event.key === ' ') {
                        event.preventDefault(); // Space 默认会滚页
                        onRowClick(row, event);
                        return;
                      }
                      const step =
                        event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
                      if (!step) return;
                      event.preventDefault();
                      // 只在同一张表内移动：分组视图有多张表，跨表跳会丢掉分组上下文
                      const siblings = Array.from(
                        event.currentTarget.parentElement?.querySelectorAll<HTMLTableRowElement>(
                          'tr[tabindex="0"]',
                        ) ?? [],
                      );
                      const at = siblings.indexOf(event.currentTarget);
                      siblings[at + step]?.focus();
                    }
                  : undefined
              }
              onContextMenu={
                onRowContextMenu ? (event) => onRowContextMenu(row, event) : undefined
              }
            >
              {lead && <td className={cn(cellCls(dense), 'whitespace-nowrap')}>{lead.cell(row)}</td>}
              {columns.map((col) => (
                <td
                  key={col.key}
                  className={cn(
                    cellCls(dense),
                    // 单行是默认：换行会把整行撑高，而同排其他列都单行
                    !col.wrap && 'whitespace-nowrap',
                    col.truncate && 'overflow-hidden text-ellipsis',
                    col.align === 'right' && 'text-right tabular-nums',
                    col.className,
                  )}
                  title={col.title?.(row)}
                >
                  {col.cell(row)}
                </td>
              ))}
              {trail && (
                <td className={cn(cellCls(dense), 'whitespace-nowrap text-right')}>
                  {trail.cell(row)}
                </td>
              )}
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
