import { Bell, CheckCircle2, ChevronRight, Pencil, Plus, Repeat, Trash2 } from 'lucide-react';
import { Button, Hint, Tag, cn } from '@/components/ui';
import type { Column, SideColumn } from '@/components/ui/table';
import type { DidaTask } from '@/lib/types';
import {
  formatDueInZone,
  isCompleted,
  itemProgress,
  priorityLabel,
  reminderLabel,
  repeatLabel,
  zoneLabel,
} from './format';

const PRIORITY_TONE: Record<number, 'bad' | 'warn' | 'accent' | 'muted'> = {
  5: 'bad',
  3: 'warn',
  1: 'accent',
  0: 'muted',
};

/** 表格行 = 任务 + 树深度（父在前、子紧随，子行缩进）。 */
export interface DidaRow {
  task: DidaTask;
  depth: number;
}

export function didaColumns(opts: {
  /** 聚合视图下标出任务属于哪个清单 */
  projectNames?: Map<string, string>;
  onToggleItem?: (task: DidaTask, index: number, done: boolean) => void;
}): Column<DidaRow>[] {
  const { projectNames, onToggleItem } = opts;
  return [
    {
      key: 'title',
      header: '任务',
      wrap: true,
      sortValue: (row) => row.task.title,
      cell: ({ task, depth }) => {
        const done = isCompleted(task);
        const progress = itemProgress(task.items);
        return (
          <div
            className="flex min-w-0 flex-col gap-0.5 py-0.5"
            style={{ paddingLeft: depth * 18 }}
          >
            <span className="flex min-w-0 items-center gap-1">
              {depth > 0 && (
                <ChevronRight
                  size={12}
                  aria-hidden
                  className="shrink-0 rotate-90 text-muted"
                />
              )}
              <span
                className={cn(
                  'truncate font-medium',
                  done ? 'text-muted line-through' : 'text-fg',
                )}
              >
                {task.title}
              </span>
              {progress.total > 0 && (
                <Hint
                  content={`清单子项 ${progress.done}/${progress.total} 已完成`}
                  focusable={false}
                >
                  <span className="shrink-0 rounded-sm bg-hover px-1 text-xs tabular-nums text-fg2">
                    {progress.done}/{progress.total}
                  </span>
                </Hint>
              )}
              {task.reminders.length > 0 && (
                <Hint content={`提醒：${reminderLabel(task.reminders[0])}`} focusable={false}>
                  <Bell size={11} aria-hidden className="shrink-0 text-info" />
                </Hint>
              )}
              {task.repeat_flag && (
                <Hint content={`重复：${repeatLabel(task.repeat_flag)}`} focusable={false}>
                  <Repeat size={11} aria-hidden className="shrink-0 text-iris" />
                </Hint>
              )}
            </span>
            {task.content && (
              <span className="truncate text-xs text-muted">{task.content}</span>
            )}
            {/* 子项直接在列表上勾选：不必为了打一个勾打开抽屉 */}
            {task.items.length > 0 && onToggleItem && (
              <ul className="mt-0.5 flex flex-col gap-0.5">
                {task.items.map((item, index) => (
                  <li key={item.id || `${task.id}-${index}`}>
                    <button
                      type="button"
                      onClick={() => onToggleItem(task, index, item.status !== 1)}
                      className="flex w-full min-w-0 items-center gap-1.5 text-left text-xs text-fg2 transition-colors duration-[var(--dur-1)] hover:text-fg"
                      aria-label={`${item.status === 1 ? '取消完成' : '完成'}子项「${item.title}」`}
                    >
                      <span
                        aria-hidden
                        className={cn(
                          'flex h-3 w-3 shrink-0 items-center justify-center rounded-[3px] border',
                          item.status === 1
                            ? 'border-ok bg-ok text-on-accent'
                            : 'border-line-strong',
                        )}
                      >
                        {item.status === 1 && <CheckCircle2 size={9} />}
                      </span>
                      <span
                        className={cn(
                          'truncate',
                          item.status === 1 && 'text-muted line-through',
                        )}
                      >
                        {item.title}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        );
      },
    },
    ...(projectNames
      ? [
          {
            key: 'project',
            header: '清单',
            width: 110,
            sortValue: (row: DidaRow) => projectNames.get(row.task.project_id) ?? '',
            cell: (row: DidaRow) => (
              <span className="truncate text-xs text-fg2">
                {projectNames.get(row.task.project_id) ?? row.task.project_id}
              </span>
            ),
          } as Column<DidaRow>,
        ]
      : []),
    {
      key: 'priority',
      header: '优先级',
      width: 76,
      sortValue: (row) => row.task.priority,
      cell: ({ task }) =>
        task.priority === 0 ? (
          <span className="text-muted">—</span>
        ) : (
          <Tag tone={PRIORITY_TONE[task.priority] ?? 'muted'}>
            {priorityLabel(task.priority)}
          </Tag>
        ),
    },
    {
      key: 'due',
      header: '截止',
      width: 150,
      sortValue: (row) => row.task.due_date ?? '',
      cell: ({ task }) => {
        const view = formatDueInZone(
          task.due_date,
          task.is_all_day,
          isCompleted(task),
          task.time_zone,
        );
        if (!view) return <span className="text-muted">—</span>;
        return (
          <Hint
            focusable={false}
            content={[
              `原始时间：${task.due_date}`,
              `时区：${zoneLabel(task.time_zone)}`,
              task.start_date ? `开始：${task.start_date}` : '',
            ]
              .filter(Boolean)
              .join('\n')}
          >
            <span
              className={cn('tabular-nums', view.overdue ? 'text-bad' : 'text-fg2')}
            >
              {view.text}
              {view.overdue ? ' 已逾期' : ''}
            </span>
          </Hint>
        );
      },
    },
    {
      key: 'tags',
      header: '标签',
      width: 130,
      cell: ({ task }) =>
        task.tags.length === 0 ? (
          <span className="text-muted">—</span>
        ) : (
          <span className="flex min-w-0 flex-wrap gap-1">
            {task.tags.slice(0, 3).map((tag) => (
              <Tag key={tag} tone="info" variant="outline">
                {tag}
              </Tag>
            ))}
            {task.tags.length > 3 && (
              <Hint content={task.tags.join('、')} focusable={false}>
                <span className="text-xs text-muted">+{task.tags.length - 3}</span>
              </Hint>
            )}
          </span>
        ),
    },
  ];
}

/** 操作列：完成 / 加子任务 / 编辑 / 删除。已完成的任务不再给「完成」。 */
export function didaTrail(
  onComplete: (task: DidaTask) => void,
  onEdit: (task: DidaTask) => void,
  onDelete: (task: DidaTask) => void,
  onAddChild?: (task: DidaTask) => void,
): SideColumn<DidaRow> {
  return {
    width: onAddChild ? 124 : 100,
    cell: ({ task }) => (
      <div className="flex items-center justify-end gap-0.5">
        {!isCompleted(task) && (
          <Hint content="标记为已完成">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-6 w-6 p-0"
              aria-label={`完成「${task.title}」`}
              onClick={() => onComplete(task)}
            >
              <CheckCircle2 size={13} />
            </Button>
          </Hint>
        )}
        {onAddChild && !isCompleted(task) && (
          <Hint content="在这个任务下新建子任务">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-6 w-6 p-0"
              aria-label={`在「${task.title}」下新建子任务`}
              onClick={() => onAddChild(task)}
            >
              <Plus size={13} />
            </Button>
          </Hint>
        )}
        <Hint content="编辑任务">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 w-6 p-0"
            aria-label={`编辑「${task.title}」`}
            onClick={() => onEdit(task)}
          >
            <Pencil size={13} />
          </Button>
        </Hint>
        <Hint content="删除任务（可撤销）">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 w-6 p-0 hover:text-bad"
            aria-label={`删除「${task.title}」`}
            onClick={() => onDelete(task)}
          >
            <Trash2 size={13} />
          </Button>
        </Hint>
      </div>
    ),
  };
}