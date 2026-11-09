// 事件传输的统一入口：桌面 WebView 走 Tauri event，网页调试模式走 SSE（/events）。
// 页面只管 listen(事件名, handler)，不感知传输差异；handler 形状与
// @tauri-apps/api/event 的 listen 完全一致（e.payload 取载荷）。
import { inTauri } from './ipc';
import { listen as tauriListen } from '@tauri-apps/api/event';

type Listener = (e: { payload: unknown }) => void;

const listeners = new Map<string, Set<Listener>>();
let source: EventSource | null = null;

/** 网页模式：整页共用一条 SSE 连接，按事件名分派。断线由浏览器自动重连。 */
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

/** 后端实时事件（桌面壳走 Tauri event，网页壳走 SSE）。返回解绑函数。 */
export async function listen<T>(
  event: string,
  handler: (e: { payload: T }) => void,
): Promise<() => void> {
  if (inTauri) {
    return tauriListen<T>(event, handler);
  }
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
