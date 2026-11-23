// 界面折叠/展开这类纯显示状态：落 localStorage，刷新与切页后保持原样。
//
// 边界：业务配置不放这里（那是 `data/config.toml` 与后端 kv 的职责）；
// 这里只存「用户把哪块收起来了」这种丢了也不影响数据正确性的偏好。
import { useCallback, useState, useSyncExternalStore } from 'react';

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

/** 字符串枚举的持久化：值不在白名单里就回退默认值，避免脏值把 UI 卡死。 */
export function parseChoice<T extends string>(
  raw: string | null,
  fallback: T,
  values: readonly T[],
): T {
  if (!raw) return fallback;
  return (values as readonly string[]).includes(raw) ? (raw as T) : fallback;
}

/**
 * 持久化的单选值（分组/排序/导航布局这类枚举）。
 *
 * 同一个 key 在多处使用时**共享一份值**：导航布局既在顶栏切换、也在设置页切换，
 * 各自 useState 会让两处显示不同步。这里按 key 建模块级快照，所有调用方订阅同一份。
 */
const choiceSlots = new Map<string, { value: string; subs: Set<() => void> }>();

function choiceSlot<T extends string>(key: string, fallback: T, values: readonly T[]) {
  let slot = choiceSlots.get(key);
  if (!slot) {
    slot = {
      value: parseChoice(store()?.getItem(PREFIX + key) ?? null, fallback, values),
      subs: new Set(),
    };
    choiceSlots.set(key, slot);
  }
  return slot;
}

export function usePersistedChoice<T extends string>(
  key: string,
  fallback: T,
  values: readonly T[],
): [T, (next: T) => void] {
  const slot = choiceSlot(key, fallback, values);
  const value = useSyncExternalStore(
    (cb) => {
      slot.subs.add(cb);
      return () => slot.subs.delete(cb);
    },
    () => slot.value,
  ) as T;
  const update = useCallback(
    (next: T) => {
      if (slot.value === next) return;
      slot.value = next;
      try {
        store()?.setItem(PREFIX + key, next);
      } catch {
        /* 写不进去就只在本次会话生效 */
      }
      for (const cb of slot.subs) cb();
    },
    [key, slot],
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

/**
 * 界面字号档位（总纲 P1-13）。纯显示偏好，落 localStorage 而非 config.toml——
 * 后者是业务配置唯一真源，字号跟机器/浏览器走更合理。
 */
export type FontChoice = 'standard' | 'large';
export const FONT_CHOICES: readonly FontChoice[] = ['standard', 'large'];
export const FONT_KEY = 'font';

/** 读档位：缺失、被策略禁用或值非法一律回退「标准」。 */
export function loadFontChoice(): FontChoice {
  return parseChoice(store()?.getItem(PREFIX + FONT_KEY) ?? null, 'standard', FONT_CHOICES);
}

/** 写档位：写不进去也返回 true 语义由调用方决定——这里只保证不抛错。 */
export function saveFontChoice(choice: FontChoice): void {
  try {
    store()?.setItem(PREFIX + FONT_KEY, choice);
  } catch {
    /* 隐私模式：只在本次会话生效 */
  }
}

/**
 * 导航布局。三种都是正式形态，设置页与顶栏都能切：
 *   sidebar 侧边栏 240px + 窄顶栏（后台管理经典结构）
 *   topbar  顶部标签导航（一屏全给内容，窗口矮时最省纵向空间）
 *   rail    56px 图标轨道，可展开成 240px（表格页要横向空间时用）
 */
export type NavLayout = 'sidebar' | 'topbar' | 'rail';
export const NAV_LAYOUTS: readonly NavLayout[] = ['sidebar', 'topbar', 'rail'];
export const NAV_KEY = 'nav';

/**
 * 启动画面的配色提示。风格真源是后端 kv，首屏那一刻还取不到，
 * 所以另存一份纯绘制用的副本给 index.html 的内联脚本读——丢了只是首屏闪一下白，
 * 不影响真正的风格选择。
 */
export function saveThemeHint(choice: string): void {
  try {
    store()?.setItem(PREFIX + 'theme', choice);
  } catch {
    /* 同上：写不进去就不提示 */
  }
}
