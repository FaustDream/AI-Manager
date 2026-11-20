// 会话视图的共享模型：分页大小、行键、状态色、两个列表的列定义。
// 活跃列表与「已归档会话」独立页共用，避免两处各写一份列定义慢慢漂移。
import type { Column } from '@/components/ui/table';
import { Tag } from '@/components/ui';
import { fmtSize, fmtTime } from '@/lib/format';
import type { SessionRow } from '@/lib/types';
import { STATE_LABELS } from './grouping';

export const PAGE_SIZE = 100;

/** 标题列里的两个小标：置顶星标（可点切换）与「本地命名」铅笔。 */
export function TitleMarks({
  row,
  onTogglePin,
}: {
  row: SessionRow;
  onTogglePin?: (row: SessionRow) => void;
}) {
  return (
    <span className="inline-flex shrink-0 items-center gap-1 align-middle">
      {row.pinned && (
        <span title="已置顶（排在列表最前）" className="text-accent">
          <StarIcon filled />
        </span>
      )}
      {row.renamed && (
        <span title="本地命名（客户端标题不变，扫描不会覆盖）" className="text-muted">
          <PencilIcon />
        </span>
      )}
      {onTogglePin && (
        <button
          type="button"
          // 悬停才显形，但**键盘也要能拿到**（P1-9）：`hidden` 会 display:none，
          // Tab 永远跳过它，等于这功能对键盘用户不存在。sr-only 保留 1px 可聚焦盒
          // （不占位、也不会误点），聚焦或悬停时还原成正常图标。
          className={
            row.pinned
              ? 'sr-only cursor-pointer text-accent group-hover:not-sr-only group-focus-within:not-sr-only focus-visible:not-sr-only'
              : 'sr-only cursor-pointer text-muted group-hover:not-sr-only group-focus-within:not-sr-only focus-visible:not-sr-only'
          }
          title={row.pinned ? '取消置顶' : '置顶（排在列表最前）'}
          aria-label={row.pinned ? '取消置顶' : '置顶'}
          aria-pressed={row.pinned}
          onClick={(e) => {
            e.stopPropagation();
            onTogglePin(row);
          }}
        >
          <StarIcon filled={row.pinned} />
        </button>
      )}
    </span>
  );
}

/** 14px 星形（lucide 的 Star 太粗，这里手画一个细线的）。 */
function StarIcon({ filled }: { filled?: boolean }) {
  return (
    <svg viewBox="0 0 16 16" width="12" height="12" fill={filled ? 'currentColor' : 'none'} aria-hidden>
      <path
        d="M8 1.5l1.9 4.1 4.4.5-3.3 3 .9 4.4L8 11.2l-3.9 2.3.9-4.4-3.3-3 4.4-.5L8 1.5z"
        stroke="currentColor"
        strokeWidth="1.1"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function PencilIcon() {
  return (
    <svg viewBox="0 0 16 16" width="12" height="12" fill="none" aria-hidden>
      <path
        d="M11.3 2.2l2.5 2.5-7.6 7.6-3.3.8.8-3.3 7.6-7.6z"
        stroke="currentColor"
        strokeWidth="1.1"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** 一行的稳定键：列表、选择集、预览都以它为准。 */
export const rowKey = (r: SessionRow) =>
  `${r.kind}|${r.source_key}|${r.account}|${r.session_id}`;

export const STATE_TONES: Record<string, 'ok' | 'warn' | 'bad' | 'muted'> = {
  visible: 'ok', archived: 'warn', deleted: 'bad', cloud: 'muted', old_account: 'muted',
  orphan: 'warn',
};

/** 主列表的列定义（C6）：加一列 = 加一行。置顶/本地命名标识画在标题格内。 */
export const SESSION_COLUMNS: Column<SessionRow>[] = [
  {
    key: 'title',
    header: '标题',
    truncate: true,
    // 正文搜索命中时把摘要带进悬浮：列表不加列，想要的人悬浮看
    title: (row) => row.snippet ?? undefined,
    cell: (row) => (
      <span className="flex min-w-0 items-center gap-1.5">
        <TitleMarks row={row} />
        <span className="truncate">{row.title || '（无标题）'}</span>
      </span>
    ),
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
  { key: 'source', header: '平台', width: 144, truncate: true, className: 'text-muted', cell: (row) => row.source_label },
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
