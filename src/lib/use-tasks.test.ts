// C3 的唯一派生口：findActiveTask 是纯函数，这是它该有的测试。
import { describe, expect, it } from 'vitest';
import { findActiveTask } from './use-tasks';
import type { TaskView } from './types';

const mk = (over: Partial<TaskView>): TaskView => ({
  id: 1,
  capability: 'sessions',
  name: '删除 3 条会话',
  state: 'running',
  progress: 0,
  message: '',
  queue_ms: null,
  rev: 1,
  ...over,
});

describe('findActiveTask', () => {
  it('只认存活状态（running/queued/cancelling）', () => {
    expect(findActiveTask([mk({ state: 'running' })], 'sessions')?.state).toBe('running');
    expect(findActiveTask([mk({ state: 'queued' })], 'sessions')?.state).toBe('queued');
    expect(findActiveTask([mk({ state: 'cancelling' })], 'sessions')?.state).toBe('cancelling');
    expect(findActiveTask([mk({ state: 'done' })], 'sessions')).toBeUndefined();
    expect(findActiveTask([mk({ state: 'failed' })], 'sessions')).toBeUndefined();
  });

  it('按 capability 过滤，不串页', () => {
    const tasks = [mk({ capability: 'account_center', name: '批量签到' })];
    expect(findActiveTask(tasks, 'sessions')).toBeUndefined();
    expect(findActiveTask(tasks, 'account_center')).toBeDefined();
  });

  it('namePrefix 是 startsWith 前缀匹配，两个签到任务互不误伤', () => {
    const tasks = [
      mk({ capability: 'account_center', name: '批量签到（5）' }),
      mk({ id: 2, capability: 'account_center', name: '签到 qoder:x' }),
    ];
    expect(findActiveTask(tasks, 'account_center', '批量签到')?.id).toBe(1);
    expect(findActiveTask(tasks, 'account_center', '签到')?.id).toBe(2);
  });

  it('空列表与 undefined 都返回 undefined', () => {
    expect(findActiveTask(undefined, 'sessions')).toBeUndefined();
    expect(findActiveTask([], 'sessions')).toBeUndefined();
  });
});
