// 事件传输的统一入口：整页共用一条 SSE 连接（/events），按事件名分派。
// 页面只管 listen(事件名, handler)，handler 形状保持 e.payload 取载荷。
// 断线由浏览器 EventSource 自动重连；事件只是刷新提示，丢几条无碍。

type Listener = (e: { payload: unknown }) => void;

const listeners = new Map<string, Set<Listener>>();
// 每个事件名对应挂在 EventSource 上的那**一个**原生监听。退订到 0 时必须凭它
// removeEventListener——否则反复挂载/卸载（如开关弹窗）会把原生监听越叠越多，
// 同一条事件触发多次，放大所有副作用（重复 toast、重复刷新）。
const nativeHandlers = new Map<string, EventListener>();
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
  // 原生监听按事件名去重：存下同一个引用，退订到 0 时才能精确摘掉它
  if (!nativeHandlers.has(event)) {
    const native: EventListener = (e) => dispatch(event, (e as MessageEvent).data);
    nativeHandlers.set(event, native);
    source!.addEventListener(event, native);
  }
  return () => {
    const current = listeners.get(event);
    if (!current) return;
    current.delete(listener);
    if (current.size === 0) {
      listeners.delete(event);
      const native = nativeHandlers.get(event);
      if (native) {
        source?.removeEventListener(event, native);
        nativeHandlers.delete(event);
      }
    }
  };
}
