// 界面折叠/展开这类纯显示状态：落 localStorage，刷新与切页后保持原样。
//
// 边界：业务配置不放这里（那是 `data/config.toml` 与后端 kv 的职责）；
// 这里只存「用户把哪块收起来了」这种丢了也不影响数据正确性的偏好。
import { useCallback, useState } from 'react';

const PREFIX = 'aim.ui.';

/** localStorage 在隐私模式或被策略禁用时会抛异常，统一降级成「不持久化」。 */
function store(): Storage | null {
  try {
    return typeof window === 'undefined' ? null : window.localStorage;
  } catch {
    return null;
  }
}

/** `"1"` / `"0"` → boolean；缺失或损坏时回退默认值。 */
export function parseFlag(raw: string | null, fallback: boolean): boolean {
  if (raw === null) return fallback;
  return raw === '1' || raw === 'true';
}

/** JSON 数组 → Set；缺失或损坏时给空集，绝不抛错打断渲染。 */
export function parseSet(raw: string | null): Set<string> {
  if (!raw) return new Set();
  try {
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value)) return new Set();
    return new Set(value.filter((item): item is string => typeof item === 'string'));
  } catch {
    return new Set();
  }
}

/** 持久化的开关。`fallback` 决定「从没设置过」时的默认状态。 */
export function usePersistedFlag(
  key: string,
  fallback: boolean,
): [boolean, (next: boolean) => void] {
  const [value, setValue] = useState(() => parseFlag(store()?.getItem(PREFIX + key) ?? null, fallback));
  const update = useCallback(
    (next: boolean) => {
      setValue(next);
      try {
        store()?.setItem(PREFIX + key, next ? '1' : '0');
      } catch {
        /* 写不进去就只在本次会话生效 */
      }
    },
    [key],
  );
  return [value, update];
}

/** 持久化的折叠键集合（分组折叠用）。 */
export function usePersistedSet(key: string): [Set<string>, (next: Set<string>) => void] {
  const [value, setValue] = useState(() => parseSet(store()?.getItem(PREFIX + key) ?? null));
  const update = useCallback(
    (next: Set<string>) => {
      setValue(next);
      try {
        store()?.setItem(PREFIX + key, JSON.stringify([...next]));
      } catch {
        /* 同上：写不进去只在本次会话生效 */
      }
    },
    [key],
  );
  return [value, update];
}
