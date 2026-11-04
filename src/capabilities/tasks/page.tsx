// 任务：后台任务列表（进度 / 取消 / 清理）。原先是设置页里的一个 tab，独立成一级能力。
import { useQueryClient } from '@tanstack/react-query';
import { runCmd, useBusy } from '@/lib/cmd';
import { useTasks } from '@/lib/use-tasks';
import { Button } from '@/components/ui/button';
import { StateBlock } from '@/components/ui/form';
import { Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';

export default function TasksPage() {
  return <TasksPane />;
}

function TasksPane() {
  const qc = useQueryClient();
  const { busy, run: runBusy } = useBusy();
  const tasks = useTasks();
  const STATE_STYLE: Record<string, string> = {
    queued: 'muted', running: 'warn', cancelling: 'warn', done: 'ok', partial: 'warn', failed: 'bad', cancelled: 'muted',
  };
  const STATE_LABEL: Record<string, string> = {
    queued: '排队中', running: '运行中', cancelling: '取消中', done: '已完成',
    partial: '部分完成', failed: '失败', cancelled: '已取消',
  };
  const CAP_LABEL: Record<string, string> = {
    account_center: '账号中心', sessions: '会话', hub: '技能 MCP',
    maintenance: '维护', settings: '设置',
  };
  const rows = tasks.data ?? [];
  const runningCount = rows.filter((t) => ['running', 'queued', 'cancelling'].includes(t.state)).length;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-3">
        <span className="text-xs text-muted">
          {runningCount > 0 ? `${runningCount} 个任务进行中` : '当前没有进行中的任务'}
        </span>
        <span className="flex-1" />
        <Button
          size="sm"
          loading={busy === 'clean'}
          disabled={!!busy}
          onClick={() =>
            runBusy('clean', async () => {
              const n = await runCmd('task_clear_finished');
              if (n === undefined) return;
              toast(`清理了 ${n} 条已结束的任务`);
              qc.invalidateQueries({ queryKey: ['tasks'] });
            })
          }
        >
          清理已结束
        </Button>
      </div>
      <div className="rounded-lg border border-line bg-card p-2">
        {rows.length === 0 && (
          <StateBlock
            kind="empty"
            title="还没有任务记录"
            detail="批量签到、会话扫描、每日轮询、积分刷新都会在这里出现，可随时取消进行中的任务。"
          />
        )}
        {rows.map((task) => {
          const alive = ['queued', 'running', 'cancelling'].includes(task.state);
          return (
            <div key={task.id} className="flex items-center gap-2 border-b border-line px-1 py-1.5 text-sm last:border-b-0">
              <span className="w-16 shrink-0 text-xs text-muted">{CAP_LABEL[task.capability] ?? task.capability}</span>
              <Tag tone={(STATE_STYLE[task.state] as 'ok' | 'warn' | 'bad' | 'muted') ?? 'muted'}>
                {STATE_LABEL[task.state] ?? task.state}
              </Tag>
              <span className="min-w-0 flex-1 truncate" title={task.message}>
                {task.name}
                {task.message && <span className="text-muted"> · {task.message}</span>}
              </span>
              {task.queue_ms !== null && task.queue_ms > 0 && (
                <span className="shrink-0 text-xs text-muted" title="排队耗时">排队 {task.queue_ms}ms</span>
              )}
              {task.progress !== null && (
                <span className="inline-block h-1.5 w-24 shrink-0 overflow-hidden rounded-full bg-hover">
                  <span className="block h-full bg-accent transition-all" style={{ width: `${Math.round(task.progress * 100)}%` }} />
                </span>
              )}
              {alive && (
                <Button
                  size="sm"
                  variant="danger"
                  onClick={async () => {
                    await runCmd('task_cancel', { taskId: task.id });
                    qc.invalidateQueries({ queryKey: ['tasks'] });
                  }}
                >
                  取消
                </Button>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
