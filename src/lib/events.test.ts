// events.ts 的回归：退订必须真正摘掉 EventSource 上的原生监听。
// 早先只从 Map 删、不 removeEventListener，反复挂载/卸载会把原生监听越叠越多，
// 同一条事件触发多次。测试环境是 node（无 jsdom），这里用桩 EventSource 顶替全局。
import { beforeEach, describe, expect, it, vi } from 'vitest';

type Handler = (e: { data: string }) => void;

class FakeEventSource {
  handlers = new Map<string, Handler[]>();
  onmessage: Handler | null = null;
  addEventListener(name: string, h: Handler): void {
    const list = this.handlers.get(name) ?? [];
    list.push(h);
    this.handlers.set(name, list);
  }
  removeEventListener(name: string, h: Handler): void {
    this.handlers.set(name, (this.handlers.get(name) ?? []).filter((x) => x !== h));
  }
  /** 模拟后端推一条命名事件。 */
  emit(name: string, data = '{}'): void {
    for (const h of [...(this.handlers.get(name) ?? [])]) h({ data });
  }
  nativeCount(name: string): number {
    return (this.handlers.get(name) ?? []).length;
  }
}

let fake: FakeEventSource;

// 每个测试都重置模块，拿到干净的 listeners / nativeHandlers / source，
// 并把桩 EventSource 装到全局；模块首次 listen 时 new 出来的实例被捕获进 fake。
beforeEach(() => {
  vi.resetModules();
  fake = new FakeEventSource();
  (globalThis as { EventSource?: unknown }).EventSource = class {
    constructor() {
      // 把桩的方法挂到这个实例上，模块拿到的 source 就是它
      Object.assign(this, {
        set onmessage(v: Handler | null) {
          fake.onmessage = v;
        },
        get onmessage() {
          return fake.onmessage;
        },
        addEventListener: (n: string, h: Handler) => fake.addEventListener(n, h),
        removeEventListener: (n: string, h: Handler) => fake.removeEventListener(n, h),
      });
    }
  };
});

async function load() {
  return await import('./events');
}

describe('listen 退订', () => {
  it('订阅→全部退订→再订阅，事件只触发一次（原生监听不叠加）', async () => {
    const { listen } = await load();
    let calls = 0;
    const un1 = await listen('evt', () => calls++);
    un1(); // 退订到 0
    expect(fake.nativeCount('evt')).toBe(0); // 原生监听已摘掉
    const un2 = await listen('evt', () => calls++);
    expect(fake.nativeCount('evt')).toBe(1); // 再订阅只挂一个
    fake.emit('evt');
    expect(calls).toBe(1);
    un2();
  });

  it('退订后不再触发，且原生监听被移除', async () => {
    const { listen } = await load();
    let calls = 0;
    const un = await listen('evt', () => calls++);
    fake.emit('evt');
    expect(calls).toBe(1);
    un();
    fake.emit('evt');
    expect(calls).toBe(1); // 不再触发
    expect(fake.nativeCount('evt')).toBe(0);
  });

  it('多个订阅者共存时只挂一个原生监听，退订一个不影响另一个', async () => {
    const { listen } = await load();
    let a = 0;
    let b = 0;
    const unA = await listen('evt', () => a++);
    await listen('evt', () => b++);
    expect(fake.nativeCount('evt')).toBe(1);
    fake.emit('evt');
    expect(a).toBe(1);
    expect(b).toBe(1);
    unA();
    expect(fake.nativeCount('evt')).toBe(1); // 还有 b，原生监听保留
    fake.emit('evt');
    expect(a).toBe(1); // a 已退订
    expect(b).toBe(2);
  });
});
