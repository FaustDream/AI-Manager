// 设置页数字输入的统一解析：非有限 / 负数一律回退默认值。
// 抽出来是因为 ConfigPane 每个数字输入都要这一步，且值得有一条测试兜底。

/**
 * 把输入框的字符串解析成非负数字；解析失败或为负时返回 fallback。
 *
 * @param value 输入框原始字符串（可能为空、带非法字符）
 * @param fallback 解析失败时的默认值（通常是当前配置值）
 */
export function numOr(value: string, fallback: number): number {
  // 空串的 Number('') 是 0：清空输入框会静默变成 0，必须按「没填」回退
  if (value.trim() === '') return fallback;
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback;
}
