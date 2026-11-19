// 批量动作的实时进度：把后端 `progress` 事件聚合成「哪颗按钮跑到第几项」。
//
// 刻意不新开命令面（ARCH-14）：进度是提示不是契约，丢了只影响读数；
// 总数也只有发起批次的后端知道（过滤完停用账号后剩几个），前端只管累加完成数。
// 事件传输仍走唯一的 events 入口，界面按动作键（`checkin_batch` 等）取自己那条。
import { useEffect, useState } from 'react';
import { listen } from './events';

export interface BatchProgress {
  action: string;
  batch: string;
  total: number;
  done: number;
  ok: number;
  skip: number;
  fail: number;
  /** `end` 到了就为 false；界面据此收掉计数，不必等按钮 loading 自己停 */
  running: boolean;
}

type Payload = {
  action: string;
  batch: string;
  phase: 'start' | 'tick' | 'end';
  total?: number;
  outcome?: 'ok' | 'skip' | 'fail';
};

const current = new Map<string, BatchProgress>();
const subs = new Set<() => void>();
let wired = false;

function notify(): void {
  for (const fn of subs) fn();
}

function wire(): void {
  if (wired) return;
  wired = true;
  void listen<Payload>('progress', (e) => {
    const p = e.payload;
    if (!p || !p.action || !p.batch) return;
    if (p.phase === 'start') {
      current.set(p.action, {
        action: p.action,
        batch: p.batch,
        total: p.total ?? 0,
        done: 0,
        ok: 0,
        skip: 0,
        fail: 0,
        running: true,
      });
    } else {
      const entry = current.get(p.action);
      // 上一批的尾巴不能算到这一批头上：批次号对不上的一律丢弃
      if (!entry || entry.batch !== p.batch) return;
      if (p.phase === 'tick') {
        entry.done += 1;
        if (p.outcome === 'ok') entry.ok += 1;
        else if (p.outcome === 'skip') entry.skip += 1;
        else entry.fail += 1;
        // 计数先到总数、end 还没来（并发收尾/事件丢条）也要及时收口
        if (entry.total > 0 && entry.done >= entry.total) entry.running = false;
      } else {
        entry.running = false;
      }
    }
    notify();
  });
}

/** 某个动作当前批次的进度；没有在跑（或从没跑过）返回 null。 */
export function useProgress(action: string): BatchProgress | null {
  wire();
  const [value, setValue] = useState<BatchProgress | null>(null);
  useEffect(() => {
    const sync = () => {
      const entry = current.get(action);
      // 复制一份再给 React：Map 里的是可变对象，直接引用会让 diff 看不出变化
      setValue(entry ? { ...entry } : null);
    };
    sync();
    subs.add(sync);
    return () => {
      subs.delete(sync);
    };
  }, [action]);
  return value;
}
