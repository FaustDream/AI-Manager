// 任务数据的唯一消费口（架构评审 C3）：轮询一处、「谁在跑」的派生一处，
// 页面不再各自 useQuery(['tasks']) + 手写状态匹配。事件桥（providers.tsx）继续喂缓存。
import { useQuery } from '@tanstack/react-query';
import { queryCmd } from './cmd';
import type { TaskView } from './types';

const ALIVE = ['running', 'queued', 'cancelling'];

export interface TasksOpts {
  enabled?: boolean;
  /** 拖拽等交互期可传 false 暂停 */
  refetchInterval?: number | false;
}

export function useTasks(opts?: TasksOpts) {
  return useQuery<TaskView[]>({
    queryKey: ['tasks'],
    queryFn: () => queryCmd('task_list'),
    refetchInterval: opts?.refetchInterval ?? 2000,
    enabled: opts?.enabled ?? true,
  });
}

/** 「这个能力当前有活干吗」的派生（纯函数，可单测）。 */
export function findActiveTask(
  tasks: TaskView[] | undefined,
  capability: string,
  namePrefix?: string,
): TaskView | undefined {
  return (tasks ?? []).find(
    (t) =>
      t.capability === capability &&
      ALIVE.includes(t.state) &&
      (namePrefix === undefined || t.name.startsWith(namePrefix)),
  );
}

export function useActiveTask(capability: string, namePrefix?: string): TaskView | undefined {
  return findActiveTask(useTasks().data, capability, namePrefix);
}
