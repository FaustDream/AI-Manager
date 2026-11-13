// 会话视图的共享模型：分页大小、行键、状态色、两个列表的列定义。
// 活跃列表与「已归档会话」独立页共用，避免两处各写一份列定义慢慢漂移。
import type { Column } from '@/components/ui/table';
import { Tag } from '@/components/ui';
import { fmtSize, fmtTime } from '@/lib/format';
import type { SessionRow } from '@/lib/types';
import { STATE_LABELS } from './grouping';

export const PAGE_SIZE = 100;

/** 一行的稳定键：列表、选择集、预览都以它为准。 */
export const rowKey = (r: SessionRow) =>
  `${r.kind}|${r.source_key}|${r.account}|${r.session_id}`;

export const STATE_TONES: Record<string, 'ok' | 'warn' | 'bad' | 'muted'> = {
  visible: 'ok', archived: 'warn', deleted: 'bad', cloud: 'muted', old_account: 'muted',
  orphan: 'warn',
};

/** 主列表的列定义（C6）：加一列 = 加一行。 */
export const SESSION_COLUMNS: Column<SessionRow>[] = [
  {
    key: 'title',
    header: '标题',
    truncate: true,
    cell: (row) => row.title || '（无标题）',
  },
  {
    key: 'project',
    header: '项目',
    width: 144,
    truncate: true,
    className: 'text-muted',
    title: (row) => row.cwd || undefined,
    cell: (row) => row.project || '—',
  },
  { key: 'source', header: '来源', width: 144, truncate: true, className: 'text-muted', cell: (row) => row.source_label },
  { key: 'account', header: '账号', width: 128, truncate: true, className: 'text-muted', cell: (row) => row.account_label },
  // truncate 不只是省略号：它还带着 whitespace-nowrap。少了它，窗口一窄这列会把
  // 「11-13 09:20」折成两行，整张表的行高被这一列撑起来（其他列都是单行）。
  // 宽度给 128 而不是 112：125% 缩放下 112 刚好差几像素，会先省略再换行。
  {
    key: 'time',
    header: '时间',
    width: 128,
    truncate: true,
    className: 'tabular-nums text-muted',
    title: (row) => fmtTime(row.last_at),
    cell: (row) => fmtTime(row.last_at),
  },
  { key: 'msgs', header: '消息', width: 80, truncate: true, className: 'tabular-nums text-muted', cell: (row) => row.msgs ?? '—' },
  { key: 'size', header: '大小', width: 96, truncate: true, className: 'tabular-nums text-muted', cell: (row) => fmtSize(row.size) },
  {
    key: 'state',
    header: '状态',
    width: 80,
    truncate: true,
    cell: (row) => (
      <Tag tone={STATE_TONES[row.state] ?? 'muted'}>{STATE_LABELS[row.state] ?? row.state}</Tag>
    ),
  },
];

/** 归档列表的列：整表都是已归档，所以去掉「状态」列，把标识收进标题——
 *  一眼看出哪些是归档件。 */
export const ARCHIVED_COLUMNS: Column<SessionRow>[] = SESSION_COLUMNS.map(
  (column): Column<SessionRow> =>
    column.key === 'title'
      ? {
          ...column,
          cell: (row: SessionRow) => (
            <span className="flex min-w-0 items-center gap-1.5">
              <Tag tone="warn">已归档</Tag>
              <span className="truncate">{row.title || '（无标题）'}</span>
            </span>
          ),
        }
      : column,
).filter((column) => column.key !== 'state');
