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

// ---------------------------------------------------------------- 时区
//
// 上面那组双向硬编码 +0800：任务自带 timeZone 时，界面会把「东京 09:00」
// 显示成「北京 09:00」，编辑保存后时间就漂了一小时。下面这组按任务时区算墙钟，
// 用 Intl 而不是静态偏移表——静态表遇到夏令时会错。

export const DEFAULT_ZONE = 'Asia/Shanghai';

export const ZONES = [
  { value: 'Asia/Shanghai', label: '北京 / 上海 (UTC+8)' },
  { value: 'Asia/Tokyo', label: '东京 (UTC+9)' },
  { value: 'Asia/Singapore', label: '新加坡 (UTC+8)' },
  { value: 'Asia/Kolkata', label: '加尔各答 (UTC+5:30)' },
  { value: 'Europe/London', label: '伦敦' },
  { value: 'Europe/Berlin', label: '柏林' },
  { value: 'America/New_York', label: '纽约' },
  { value: 'America/Los_Angeles', label: '洛杉矶' },
  { value: 'UTC', label: 'UTC' },
];

export function zoneLabel(zone: string | null): string {
  if (!zone) return ZONES[0].label;
  return ZONES.find((z) => z.value === zone)?.label ?? zone;
}

interface Wall {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
}

/** 绝对毫秒 → 指定时区的墙钟。时区名非法时退回默认时区，不抛。 */
function wallIn(epoch: number, timeZone: string): Wall {
  const fmt = (zone: string) =>
    new Intl.DateTimeFormat('en-CA', {
      timeZone: zone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    }).formatToParts(new Date(epoch));
  let parts: Intl.DateTimeFormatPart[];
  try {
    parts = fmt(timeZone);
  } catch {
    parts = fmt(DEFAULT_ZONE);
  }
  const get = (type: string) => Number(parts.find((p) => p.type === type)?.value ?? 0);
  return {
    year: get('year'),
    month: get('month'),
    day: get('day'),
    // 部分实现把午夜给成 24
    hour: get('hour') % 24,
    minute: get('minute'),
  };
}

/** 指定时区在某一时刻的偏移（毫秒）：把墙钟当作 UTC 再减真实时刻反推。 */
function zoneOffset(epoch: number, timeZone: string): number {
  const w = wallIn(epoch, timeZone);
  const asUtc = Date.UTC(w.year, w.month - 1, w.day, w.hour, w.minute);
  return asUtc - Math.floor(epoch / 60000) * 60000;
}

const wallText = (w: Wall) =>
  `${w.year}-${pad(w.month)}-${pad(w.day)}T${pad(w.hour)}:${pad(w.minute)}`;

const isoOffset = (offsetMs: number) => {
  const sign = offsetMs < 0 ? '-' : '+';
  const abs = Math.abs(offsetMs) / 60000;
  return `${sign}${pad(Math.floor(abs / 60))}${pad(abs % 60)}`;
};

/**
 * 编辑器 datetime-local（无时区墙钟）→ 滴答 ISO 串，按给定任务时区解释。
 * 两遍求偏移：先按初值猜一个时刻取偏移，再用该偏移回算，跨夏令时边界也稳。
 */
export function toDidaInZone(value: string, allDay: boolean, timeZone: string): string | null {
  const text = value.trim();
  if (!text) return null;
  const [y, m, d] = text.slice(0, 10).split('-').map(Number);
  if (!y || !m || !d) return null;
  const allDayTime = text.length >= 16 ? text.slice(11, 16) : '00:00';
  const [hh, mm] = (allDay ? '00:00' : allDayTime).split(':').map(Number);
  const hour = hh || 0;
  const minute = mm || 0;
  const naive = Date.UTC(y, m - 1, d, hour, minute);
  let offset = zoneOffset(naive, timeZone);
  offset = zoneOffset(naive - offset, timeZone);
  return `${wallText({ year: y, month: m, day: d, hour, minute })}:00${isoOffset(offset)}`;
}

/** 滴答 ISO 串 → 编辑器 datetime-local，按任务时区（缺省北京）解释。 */
export function fromDidaInZone(value: string | null, timeZone: string | null): string {
  if (!value) return '';
  const epoch = dueEpoch(value);
  if (epoch === null) return '';
  return wallText(wallIn(epoch, timeZone || DEFAULT_ZONE));
}

/** 表格展示：按任务时区渲染，逾期判定仍用绝对时刻。 */
export function formatDueInZone(
  due: string | null,
  allDay: boolean,
  completed: boolean,
  timeZone: string | null,
): DueView | null {
  if (!due) return null;
  const epoch = dueEpoch(due);
  if (epoch === null) return { text: due, overdue: false };
  const w = wallIn(epoch, timeZone || DEFAULT_ZONE);
  const text = allDay
    ? `${w.month}月${w.day}日`
    : `${w.month}月${w.day}日 ${pad(w.hour)}:${pad(w.minute)}`;
  return { text, overdue: !completed && epoch < Date.now() };
}

// ---------------------------------------------------------------- 提醒与重复

export const REMINDERS = [
  { value: '', label: '不提醒' },
  { value: 'TRIGGER:PT0S', label: '准时' },
  { value: 'TRIGGER:-PT5M', label: '提前 5 分钟' },
  { value: 'TRIGGER:-PT15M', label: '提前 15 分钟' },
  { value: 'TRIGGER:-PT30M', label: '提前 30 分钟' },
  { value: 'TRIGGER:-PT1H', label: '提前 1 小时' },
  { value: 'TRIGGER:-PT2H', label: '提前 2 小时' },
  { value: 'TRIGGER:-P1D', label: '提前 1 天' },
  { value: 'TRIGGER:-P2D', label: '提前 2 天' },
];

export const REPEATS = [
  { value: '', label: '不重复' },
  { value: 'RRULE:FREQ=DAILY;INTERVAL=1', label: '每天' },
  { value: 'RRULE:FREQ=WEEKLY;INTERVAL=1', label: '每周' },
  { value: 'RRULE:FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,TU,WE,TH,FR', label: '工作日' },
  { value: 'RRULE:FREQ=MONTHLY;INTERVAL=1', label: '每月' },
  { value: 'RRULE:FREQ=YEARLY;INTERVAL=1', label: '每年' },
];

export function reminderLabel(trigger: string): string {
  return REMINDERS.find((r) => r.value === trigger)?.label ?? trigger;
}

export function repeatLabel(flag: string | null): string {
  if (!flag) return '';
  return REPEATS.find((r) => r.value === flag)?.label ?? '自定义重复';
}

// ---------------------------------------------------------------- 父子树

export interface TaskNode<T> {
  task: T;
  children: TaskNode<T>[];
  depth: number;
}

interface TreeTask {
  id: string;
  parent_id: string;
  sort_order: number;
}

/**
 * 按 parent_id 组树。父任务不在当前结果里（被筛掉、或跨清单）时子任务按顶层显示——
 * 不能因为找不到父就把任务藏起来，那等于数据丢失。环引用做兜底，不无限递归。
 */
export function buildTree<T extends TreeTask>(tasks: T[]): TaskNode<T>[] {
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const kidsOf = new Map<string, T[]>();
  const roots: T[] = [];
  for (const task of tasks) {
    const parent = task.parent_id;
    if (parent && byId.has(parent) && parent !== task.id) {
      const list = kidsOf.get(parent) ?? [];
      list.push(task);
      kidsOf.set(parent, list);
    } else {
      roots.push(task);
    }
  }
  const bySort = (a: T, b: T) => a.sort_order - b.sort_order;
  roots.sort(bySort);
  const seen = new Set<string>();
  const walk = (task: T, depth: number): TaskNode<T> | null => {
    if (seen.has(task.id)) return null;
    seen.add(task.id);
    const node: TaskNode<T> = { task, children: [], depth };
    for (const child of (kidsOf.get(task.id) ?? []).slice().sort(bySort)) {
      const childNode = walk(child, depth + 1);
      if (childNode) node.children.push(childNode);
    }
    return node;
  };
  const out: TaskNode<T>[] = [];
  for (const root of roots) {
    const node = walk(root, 0);
    if (node) out.push(node);
  }
  // 环里的任务不会被 roots 走到：补在末尾，保证一条都不丢
  for (const task of tasks) {
    if (seen.has(task.id)) continue;
    const node = walk(task, 0);
    if (node) out.push(node);
  }
  return out;
}

/** 压平成表格行（父在前、子紧随），带缩进深度。 */
export function flattenTree<T extends TreeTask>(tasks: T[]): { task: T; depth: number }[] {
  const out: { task: T; depth: number }[] = [];
  const visit = (nodes: TaskNode<T>[]) => {
    for (const node of nodes) {
      out.push({ task: node.task, depth: node.depth });
      visit(node.children);
    }
  };
  visit(buildTree(tasks));
  return out;
}

/** 子任务勾选进度：列表上的「2/5」小标。 */
export function itemProgress(items: { status: number }[]): { done: number; total: number } {
  return { done: items.filter((i) => i.status === 1).length, total: items.length };
}
