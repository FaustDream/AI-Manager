// 事件传输的统一入口：整页共用一条 SSE 连接（/events），按事件名分派。
// 页面只管 listen(事件名, handler)，handler 形状保持 e.payload 取载荷。
// 断线由浏览器 EventSource 自动重连；事件只是刷新提示，丢几条无碍。

type Listener = (e: { payload: unknown }) => void;

const listeners = new Map<string, Set<Listener>>();
let source: EventSource | null = null;

function ensureSource(): void {
  if (source) return;
  source = new EventSource('/events');
  source.onmessage = () => {}; // 后端只发命名事件，默认消息不用管
}

function dispatch(event: string, data: string): void {
  let payload: unknown;
  try {
    payload = JSON.parse(data) as unknown;
  } catch {
    payload = data;
  }
  for (const listener of listeners.get(event) ?? []) {
    listener({ payload });
  }
}

/** 后端实时事件（SSE）。返回解绑函数。 */
export async function listen<T>(
  event: string,
  handler: (e: { payload: T }) => void,
): Promise<() => void> {
  ensureSource();
  const set = listeners.get(event) ?? new Set<Listener>();
  const listener = handler as Listener;
  set.add(listener);
  listeners.set(event, set);
  if (set.size === 1) {
    source!.addEventListener(event, (e) => dispatch(event, (e as MessageEvent).data));
  }
  return () => {
    const current = listeners.get(event);
    if (!current) return;
    current.delete(listener);
    if (current.size === 0) listeners.delete(event);
  };
}
