// 展示格式化：时间一律本地时区紧凑格式（契约 18），大小 / 金额给人读。
export function fmtTime(iso: string | number | null | undefined): string {
  if (iso === null || iso === undefined || iso === '') return '—';
  const date = typeof iso === 'number' ? new Date(iso) : new Date(iso);
  if (Number.isNaN(date.getTime())) return '—';
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function fmtSize(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export function fmtAmount(value: number | null | undefined): string {
  if (value === null || value === undefined) return '—';
  const abs = Math.abs(value);
  if (abs >= 1e8) return `${(value / 1e8).toFixed(2)}亿`;
  if (abs >= 1e4) return `${(value / 1e4).toFixed(2)}万`;
  return value % 1 === 0 ? String(value) : value.toFixed(2);
}

export function fmtDelta(value: number | null | undefined): string {
  if (value === null || value === undefined) return '—';
  const sign = value > 0 ? '+' : '';
  return `${sign}${fmtAmount(value)}`;
}

/** 只到日：到期日这类信息看「哪一天」就够，带时分反而占地方。 */
export function fmtDay(iso: string | number | null | undefined): string {
  if (iso === null || iso === undefined || iso === '') return '—';
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '—';
  const pad = (n: number) => String(n).padStart(2, '0');
  const md = `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  return date.getFullYear() === new Date().getFullYear()
    ? md
    : `${date.getFullYear()}-${md}`;
}

/** 完整年月日：凭据到期这类「跨年也要看清」的日期，别省年份。 */
export function fmtFullDay(iso: string | number | null | undefined): string {
  if (iso === null || iso === undefined || iso === '') return '—';
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '—';
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** 距今天还有几天（按自然日算，今天到期 = 0，已过期 < 0）。 */
export function daysUntil(iso: string | number | null | undefined): number | null {
  if (iso === null || iso === undefined || iso === '') return null;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  const start = new Date();
  start.setHours(0, 0, 0, 0);
  const end = new Date(date);
  end.setHours(0, 0, 0, 0);
  return Math.round((end.getTime() - start.getTime()) / 86_400_000);
}
