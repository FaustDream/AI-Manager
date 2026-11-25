import { CheckCircle2, Pencil, Trash2 } from 'lucide-react';
import { Button, Tag, cn } from '@/components/ui';
import type { Column, SideColumn } from '@/components/ui/table';
import type { DidaTask } from '@/lib/types';
import { formatDue, isCompleted, priorityLabel } from './format';

const PRIORITY_TONE: Record<number, 'bad' | 'warn' | 'accent' | 'muted'> = {
  5: 'bad',
  3: 'warn',
  1: 'accent',
  0: 'muted',
};

export function didaColumns(): Column<DidaTask>[] {
  return [
    {
      key: 'title',
      header: '任务',
      wrap: true,
      sortValue: (row) => row.title,
      cell: (row) => (
        <div className="flex min-w-0 flex-col gap-0.5 py-0.5">
          <span
            className={cn(
              'truncate font-medium',
              isCompleted(row) ? 'text-muted line-through' : 'text-fg',
            )}
            title={row.title}
          >
            {row.title}
          </span>
          {row.content && (
            <span className="truncate text-xs text-muted" title={row.content}>
              {row.content}
            </span>
          )}
        </div>
      ),
    },
    {
      key: 'priority',
      header: '优先级',
      width: 76,
      sortValue: (row) => row.priority,
      cell: (row) =>
        row.priority === 0 ? (
          <span className="text-muted">—</span>
        ) : (
          <Tag tone={PRIORITY_TONE[row.priority] ?? 'muted'}>{priorityLabel(row.priority)}</Tag>
        ),
    },
    {
      key: 'due',
      header: '截止',
      width: 128,
      sortValue: (row) => row.due_date ?? '',
      cell: (row) => {
        const view = formatDue(row.due_date, row.is_all_day, isCompleted(row));
        if (!view) return <span className="text-muted">—</span>;
        return (
          <span
            className={cn('tabular-nums', view.overdue ? 'text-bad' : 'text-fg2')}
            title={row.due_date ?? undefined}
          >
            {view.text}
            {view.overdue ? ' 已逾期' : ''}
          </span>
        );
      },
    },
  ];
}

/** 操作列：完成 / 编辑 / 删除。已完成的任务不再给「完成」，避免点了没反应。 */
export function didaTrail(
  onComplete: (task: DidaTask) => void,
  onEdit: (task: DidaTask) => void,
  onDelete: (task: DidaTask) => void,
): SideColumn<DidaTask> {
  return {
    width: 100,
    cell: (row) => (
      <div className="flex items-center justify-end gap-0.5">
        {!isCompleted(row) && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 w-6 p-0"
            aria-label="完成"
            title="标记为已完成"
            onClick={() => onComplete(row)}
          >
            <CheckCircle2 size={13} />
          </Button>
        )}
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-6 w-6 p-0"
          aria-label="编辑"
          title="编辑任务"
          onClick={() => onEdit(row)}
        >
          <Pencil size={13} />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-6 w-6 p-0 hover:text-bad"
          aria-label="删除"
          title="删除任务"
          onClick={() => onDelete(row)}
        >
          <Trash2 size={13} />
        </Button>
      </div>
    ),
  };
}
