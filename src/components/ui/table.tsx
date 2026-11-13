// 列定义驱动的表格（C6）：表头、行、空态、骨架与截断样式收在一处，
// 页面只回答「每列渲染什么」——加一列 = 加一行定义。
// 账号中心（capabilities/account-center/columns.ts）是这套范式的蓝本，
// 这里把它下沉成可复用的表体；账号中心自己的表带拖拽/列宽调整，仍保留原实现。
import type { CSSProperties, MouseEvent, ReactNode } from 'react';
import { cn } from './button';
import { SkeletonRows, StateBlock } from './form';

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
  /** 行点击：带原生事件，页面可据此判断 Ctrl / Cmd 做多选（不传事件就没法区分） */
  onRowClick?: (row: R, event: MouseEvent<HTMLTableRowElement>) => void;
  rowClassName?: (row: R) => string;
  stickyHead?: boolean;
  /** 紧凑密度（hub 系列）：px-2 py-1 / px-2 py-1.5；默认 px-2.5 */
  dense?: boolean;
  className?: string;
  style?: CSSProperties;
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
  rowClassName,
  stickyHead,
  dense,
  className,
  style,
}: DataTableProps<R>) {
  const total = columns.length + (lead ? 1 : 0) + (trail ? 1 : 0);
  return (
    <table className={cn('w-full border-collapse', className)} style={style}>
      <thead className={stickyHead ? 'sticky top-0 bg-bg' : undefined}>
        <tr>
          {lead && (
            <th className={headCls(dense)} style={widthStyle(lead.width)}>
              {lead.head}
            </th>
          )}
          {columns.map((col) => (
            <th
              key={col.key}
              className={cn(headCls(dense), col.align === 'right' && 'text-right')}
              style={widthStyle(col.width)}
            >
              {col.header}
            </th>
          ))}
          {trail && (
            <th className={cn(headCls(dense), 'text-right')} style={widthStyle(trail.width)}>
              {trail.head}
            </th>
          )}
        </tr>
      </thead>
      <tbody>
        {loading && <SkeletonRows rows={skeletonRows} cols={total} />}
        {!loading && rows.length === 0 && empty !== null && (
          <tr>
            <td colSpan={total}>
              {empty ?? <StateBlock kind="empty" title="没有记录" />}
            </td>
          </tr>
        )}
        {rows.map((row) => (
          <tr
            key={rowKey(row)}
            className={rowClassName?.(row)}
            onClick={onRowClick ? (event) => onRowClick(row, event) : undefined}
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
              <td className={cn(cellCls(dense), 'whitespace-nowrap text-right')}>{trail.cell(row)}</td>
            )}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
