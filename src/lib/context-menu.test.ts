// 右键菜单守卫：开着时必须 preventDefault + stopPropagation，关掉时不得注册监听。
import { describe, expect, it, vi } from 'vitest';
import { installContextMenuGuard, type ContextMenuTarget } from './context-menu';

function makeTarget() {
  const handlers = new Map<string, (event: Event) => void>();
  const target: ContextMenuTarget & { handlers: typeof handlers } = {
    handlers,
    addEventListener: (type, listener) => {
      handlers.set(type, listener);
    },
    removeEventListener: (type, listener) => {
      if (handlers.get(type) === listener) handlers.delete(type);
    },
  };
  return target;
}

describe('installContextMenuGuard', () => {
  it('在捕获阶段拦截右键：默认行为与冒泡都被阻止', () => {
    const target = makeTarget();
    installContextMenuGuard(true, target);

    const handler = target.handlers.get('contextmenu');
    expect(handler).toBeTypeOf('function');

    const event = {
      preventDefault: vi.fn(),
      stopPropagation: vi.fn(),
    } as unknown as Event;
    handler!(event);
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    expect(event.stopPropagation).toHaveBeenCalledTimes(1);
  });

  it('传 false 时不注册任何监听，原生菜单照旧', () => {
    const target = makeTarget();
    installContextMenuGuard(false, target);
    expect(target.handlers.size).toBe(0);
  });

  it('卸载函数会移除监听，便于 HMR 或临时恢复', () => {
    const target = makeTarget();
    const dispose = installContextMenuGuard(true, target);
    expect(target.handlers.has('contextmenu')).toBe(true);
    dispose();
    expect(target.handlers.has('contextmenu')).toBe(false);
  });
});
