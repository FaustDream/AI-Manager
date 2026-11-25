// 滴答清单的时间与优先级纯函数：表格与编辑器共用，时区约定由单测钉住。

export const PRIORITIES = [
  { value: 0, label: '无' },
  { value: 1, label: '低' },
  { value: 3, label: '中' },
  { value: 5, label: '高' },
];

export function priorityLabel(priority: number): string {
  return PRIORITIES.find((item) => item.value === priority)?.label ?? '无';
}

export function isCompleted(task: { status: number }): boolean {
  return task.status === 2;
}

/** datetime-local 不带时区：按需求固定成 +0800，后端原样转给滴答。 */
export function toDidaDue(value: string, allDay: boolean): string | null {
  const text = value.trim();
  if (!text) return null;
  if (allDay) return `${text.slice(0, 10)}T00:00:00+0800`;
  const base = text.length === 16 ? `${text}:00` : text;
  return `${base}+0800`;
}

const DIDA_DUE =
  /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?(?:\.\d+)?(?:([+-])(\d{2}):?(\d{2})|Z)?$/;

/** 滴答时间戳 → 绝对毫秒；偏移按 ISO 自己算，不交给浏览器（避免被本机时区换算）。 */
function dueEpoch(value: string): number | null {
  const m = DIDA_DUE.exec(value.trim());
  if (!m) return null;
  const utc = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]));
  if (!m[7]) return utc;
  const offset = (Number(m[8]) * 60 + Number(m[9])) * 60000 * (m[7] === '-' ? -1 : 1);
  return utc - offset;
}

const pad = (n: number) => String(n).padStart(2, '0');

/** 展示与编辑统一 +0800 墙钟：需求固定时区，界面不随机器时区漂移。 */
function wall800(epoch: number) {
  const dt = new Date(epoch + 8 * 3600000);
  return {
    year: dt.getUTCFullYear(),
    month: dt.getUTCMonth() + 1,
    day: dt.getUTCDate(),
    hour: dt.getUTCHours(),
    minute: dt.getUTCMinutes(),
  };
}

/** 滴答时间戳 → 编辑器 datetime-local 值；解析不出就留空，不把脏串塞进控件。 */
export function fromDidaDue(value: string | null): string {
  if (!value) return '';
  const epoch = dueEpoch(value);
  if (epoch === null) return '';
  const w = wall800(epoch);
  return `${w.year}-${pad(w.month)}-${pad(w.day)}T${pad(w.hour)}:${pad(w.minute)}`;
}

export interface DueView {
  text: string;
  overdue: boolean;
}

/** 表格展示：全天只显示日期，其余到分钟；逾期只对未完成任务成立。 */
export function formatDue(due: string | null, allDay: boolean, completed: boolean): DueView | null {
  if (!due) return null;
  const epoch = dueEpoch(due);
  if (epoch === null) return { text: due, overdue: false };
  const w = wall800(epoch);
  const text = allDay
    ? `${w.month}月${w.day}日`
    : `${w.month}月${w.day}日 ${pad(w.hour)}:${pad(w.minute)}`;
  return { text, overdue: !completed && epoch < Date.now() };
}
