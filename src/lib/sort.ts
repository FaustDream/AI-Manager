// 列表的排序与关键词匹配：纯函数，vitest 只跑这一层（不碰 DOM）。
//
// 为什么自己写而不引表库：仓库规则（DEP-08）要求 UI 控件只用 `components/ui` 里的现成件，
// 不引第二套表格实现。排序本身只有「三态切换 + 稳定排序 + 空值沉底」三件事，
// 二十行就能写完，且能被单测钉死。

/** 排序方向。三态的第三态不是 `none`，而是「取消排序，回到调用方给的原始顺序」。 */
export type SortDir = 'asc' | 'desc';

export interface SortState {
  key: string;
  dir: SortDir;
}

/**
 * 点同一个表头时的三态循环：无 → 升 → 降 → 无。
 *
 * 点另一列直接回到升序（而不是继承上一次的方向）：用户点新列的意图通常是
 * 「先按它从小到大看一眼」，继承方向会让「点哪列都先降序」这种反直觉行为出现。
 */
export function nextSort(current: SortState | null, key: string): SortState | null {
  if (!current || current.key !== key) return { key, dir: 'asc' };
  return current.dir === 'asc' ? { key, dir: 'desc' } : null;
}

/** 可以拿来排序的值：数字按大小、字符串按本地化比较（`zh-Hans-CN` 下「账号 2」<「账号 10」）。 */
export type SortValue = string | number | null | undefined;

/**
 * 稳定排序，**空值一律沉底**（升序降序都一样）。
 *
 * 空值包括 null / undefined / 空串 / NaN。让它们顶在第一屏是列表最常见的毛病：
 * 「没有到期时间」的账号挤在最前面，看起来像坏了。
 */
export function sortRows<T>(
  rows: T[],
  sort: SortState | null,
  valueOf: (row: T, key: string) => SortValue,
): T[] {
  if (!sort) return rows;
  const dir = sort.dir === 'asc' ? 1 : -1;
  return [...rows].sort((a, b) => {
    const av = valueOf(a, sort.key);
    const bv = valueOf(b, sort.key);
    const aEmpty = isEmpty(av);
    const bEmpty = isEmpty(bv);
    if (aEmpty || bEmpty) {
      // 两边都空就保持原序（稳定），只有一边空就沉底
      return aEmpty === bEmpty ? 0 : aEmpty ? 1 : -1;
    }
    if (typeof av === 'number' && typeof bv === 'number') return (av - bv) * dir;
    return String(av).localeCompare(String(bv), 'zh-Hans-CN', { numeric: true }) * dir;
  });
}

function isEmpty(value: SortValue): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'number') return Number.isNaN(value);
  return value.trim() === '';
}

/**
 * 关键词匹配：**多关键词按 AND**，全角空格也当分隔符。
 *
 * 输入里通常会带昵称、平台名、UID、备注等多种字段——把它们拼成一段文本再匹配，
 * 用户敲「work 过期」也能同时命中这两条线索，不用先想清楚该搜哪个字段。
 */
export function matchesQuery(haystack: Array<string | null | undefined>, query: string): boolean {
  const words = query
    .toLowerCase()
    .split(/[\s\u3000]+/)
    .filter(Boolean);
  if (words.length === 0) return true;
  const text = haystack.filter(Boolean).join(' ').toLowerCase();
  return words.every((word) => text.includes(word));
}
