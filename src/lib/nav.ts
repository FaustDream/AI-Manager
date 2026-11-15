// 能力页之间的跳转参数通道。`aim:navigate` 只带页面 id（Shell 的约定），
// 需要携带预设条件（账号下钻、快搜定位）时走这里：
//   - 预设条件存 sessionStorage，目标页挂载时取走（一次性，读完即删）；
//   - 同页内的定位（命令面板选会话）走独立事件，不经过 Shell。
const PRESET_PREFIX = 'aim.navPreset:';

/** 跳转到能力页并可携带一次性预设（目标页用 takePreset 取）。 */
export function navigate(id: string, preset?: unknown): void {
  try {
    if (preset !== undefined) {
      sessionStorage.setItem(PRESET_PREFIX + id, JSON.stringify(preset));
    } else {
      sessionStorage.removeItem(PRESET_PREFIX + id);
    }
  } catch {
    /* sessionStorage 被禁时降级为不带预设的跳转 */
  }
  window.dispatchEvent(new CustomEvent('aim:navigate', { detail: id }));
}

/** 目标页取走预设（一次性：读完即删，避免下次进页还带着旧条件）。 */
export function takePreset<T>(id: string): T | null {
  try {
    const raw = sessionStorage.getItem(PRESET_PREFIX + id);
    if (raw === null) return null;
    sessionStorage.removeItem(PRESET_PREFIX + id);
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

/** 命令面板 / 快搜定位到某条会话（会话页挂载时监听）。 */
export function focusSession(key: string): void {
  window.dispatchEvent(new CustomEvent('aim:sessions-focus', { detail: key }));
}

/** 订阅会话定位事件，返回退订函数。 */
export function onFocusSession(handler: (key: string) => void): () => void {
  const fn = (event: Event) => handler((event as CustomEvent<string>).detail);
  window.addEventListener('aim:sessions-focus', fn);
  return () => window.removeEventListener('aim:sessions-focus', fn);
}
