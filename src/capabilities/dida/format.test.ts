import { describe, expect, it } from 'vitest';
import {
  flattenTree,
  formatDue,
  formatDueInZone,
  fromDidaDue,
  fromDidaInZone,
  isCompleted,
  itemProgress,
  priorityLabel,
  reminderLabel,
  repeatLabel,
  toDidaDue,
  toDidaInZone,
} from './format';

describe('toDidaDue', () => {
  it('补秒并固定 +0800', () => {
    expect(toDidaDue('2026-11-24T20:00', false)).toBe('2026-11-24T20:00:00+0800');
  });

  it('全天只保留日期', () => {
    expect(toDidaDue('2026-11-24T20:00', true)).toBe('2026-11-24T00:00:00+0800');
  });

  it('留空即无截止', () => {
    expect(toDidaDue('', false)).toBeNull();
  });
});

describe('fromDidaDue', () => {
  it('截到分钟回填控件', () => {
    expect(fromDidaDue('2026-11-24T20:00:00+0800')).toBe('2026-11-24T20:00');
  });

  it('其它偏移换算到 +0800 墙钟', () => {
    expect(fromDidaDue('2026-11-25T10:00:00+0000')).toBe('2026-11-25T18:00');
  });

  it('带毫秒的回执同样能解析', () => {
    expect(fromDidaDue('2026-11-25T10:00:00.000+0000')).toBe('2026-11-25T18:00');
  });

  it('空值与脏串都留空', () => {
    expect(fromDidaDue(null)).toBe('');
    expect(fromDidaDue('不是时间')).toBe('');
  });
});

describe('formatDue', () => {
  it('全天只显示日期', () => {
    const view = formatDue('2026-11-24T00:00:00+0800', true, false);
    expect(view?.text).toBe('11月24日');
  });

  it('非全天到分钟', () => {
    const view = formatDue('2026-11-24T20:05:00+0800', false, false);
    expect(view?.text).toBe('11月24日 20:05');
  });

  it('UTC 偏移换算到 +0800 墙钟', () => {
    const view = formatDue('2026-11-25T10:00:00+0000', false, false);
    expect(view?.text).toBe('11月25日 18:00');
  });

  it('带毫秒的 UTC 回执换算到 +0800 墙钟', () => {
    const view = formatDue('2026-11-25T10:00:00.000+0000', false, false);
    expect(view?.text).toBe('11月25日 18:00');
  });

  it('过去的未完成任务是逾期，已完成不算', () => {
    expect(formatDue('2020-01-01T10:00:00+0800', false, false)?.overdue).toBe(true);
    expect(formatDue('2020-01-01T10:00:00+0800', false, true)?.overdue).toBe(false);
  });

  it('无截止返回 null', () => {
    expect(formatDue(null, false, false)).toBeNull();
  });
});

it('priorityLabel 兜底为无', () => {
  expect(priorityLabel(3)).toBe('中');
  expect(priorityLabel(9)).toBe('无');
});

it('isCompleted 只认状态 2', () => {
  expect(isCompleted({ status: 2 })).toBe(true);
  expect(isCompleted({ status: 0 })).toBe(false);
});

// ---------------------------------------------------------------- 按任务时区

describe('按任务时区换算', () => {
  it('东京任务显示东京墙钟，不再被当成北京时间', () => {
    // 绝对时刻 = 北京 09:00 = 东京 10:00
    const iso = '2026-11-25T09:00:00+0800';
    expect(formatDueInZone(iso, false, false, 'Asia/Tokyo')?.text).toBe('11月25日 10:00');
    expect(formatDueInZone(iso, false, false, 'Asia/Shanghai')?.text).toBe('11月25日 09:00');
    expect(fromDidaInZone(iso, 'Asia/Tokyo')).toBe('2026-11-25T10:00');
  });

  it('墙钟按任务时区回写，往返不漂', () => {
    const tokyo = toDidaInZone('2026-11-25T10:00', false, 'Asia/Tokyo');
    expect(tokyo).toBe('2026-11-25T10:00:00+0900');
    expect(fromDidaInZone(tokyo, 'Asia/Tokyo')).toBe('2026-11-25T10:00');
    // 同一绝对时刻在北京时区读出来是 09:00
    expect(fromDidaInZone(tokyo, 'Asia/Shanghai')).toBe('2026-11-25T09:00');
  });

  it('半小时偏移时区也正确', () => {
    const kolkata = toDidaInZone('2026-11-25T10:00', false, 'Asia/Kolkata');
    expect(kolkata).toBe('2026-11-25T10:00:00+0530');
    expect(fromDidaInZone(kolkata, 'Asia/Kolkata')).toBe('2026-11-25T10:00');
  });

  it('夏令时按当日真实偏移算，不用静态表', () => {
    // 纽约 7 月是 EDT(-0400)，1 月是 EST(-0500)
    expect(toDidaInZone('2026-07-15T10:00', false, 'America/New_York')).toBe(
      '2026-07-15T10:00:00-0400',
    );
    expect(toDidaInZone('2026-01-15T10:00', false, 'America/New_York')).toBe(
      '2026-01-15T10:00:00-0500',
    );
  });

  it('全天任务只取日期，时刻归零', () => {
    expect(toDidaInZone('2026-11-25T18:30', true, 'Asia/Shanghai')).toBe(
      '2026-11-25T00:00:00+0800',
    );
  });

  it('非法时区退回默认而不是抛错', () => {
    expect(fromDidaInZone('2026-11-25T09:00:00+0800', 'Not/AZone')).toBe('2026-11-25T09:00');
  });

  it('空值与脏串不进控件', () => {
    expect(toDidaInZone('', false, 'Asia/Shanghai')).toBeNull();
    expect(fromDidaInZone(null, 'Asia/Shanghai')).toBe('');
    expect(fromDidaInZone('不是时间', 'Asia/Shanghai')).toBe('');
  });
});

// ---------------------------------------------------------------- 父子树

const node = (id: string, parent_id = '', sort_order = 0) => ({ id, parent_id, sort_order });

describe('父子树', () => {
  it('父在前子紧随，深度正确', () => {
    const rows = flattenTree([
      node('c', 'p', 1),
      node('p', '', 0),
      node('other', '', 1),
    ]);
    expect(rows.map((r) => [r.task.id, r.depth])).toEqual([
      ['p', 0],
      ['c', 1],
      ['other', 0],
    ]);
  });

  it('多层嵌套按 sortOrder 排', () => {
    const rows = flattenTree([
      node('g', 'c'),
      node('c', 'p', 2),
      node('c2', 'p', 1),
      node('p'),
    ]);
    expect(rows.map((r) => [r.task.id, r.depth])).toEqual([
      ['p', 0],
      ['c2', 1],
      ['c', 1],
      ['g', 2],
    ]);
  });

  it('父任务被筛掉时子任务按顶层显示，不丢数据', () => {
    const rows = flattenTree([node('c', 'missing')]);
    expect(rows).toHaveLength(1);
    expect(rows[0].depth).toBe(0);
  });

  it('环引用不死循环且不丢任务', () => {
    const rows = flattenTree([node('a', 'b'), node('b', 'a')]);
    expect(rows.map((r) => r.task.id).sort()).toEqual(['a', 'b']);
  });

  it('自引用按顶层处理', () => {
    const rows = flattenTree([node('a', 'a')]);
    expect(rows).toHaveLength(1);
    expect(rows[0].depth).toBe(0);
  });

  it('子任务进度只数 status===1', () => {
    expect(itemProgress([{ status: 1 }, { status: 0 }, { status: 1 }])).toEqual({
      done: 2,
      total: 3,
    });
    expect(itemProgress([])).toEqual({ done: 0, total: 0 });
  });
});

describe('提醒与重复文案', () => {
  it('已知触发串给中文，未知原样保留', () => {
    expect(reminderLabel('TRIGGER:-PT15M')).toBe('提前 15 分钟');
    expect(reminderLabel('TRIGGER:-PT7M')).toBe('TRIGGER:-PT7M');
  });
  it('重复规则给中文，自定义不假装认识', () => {
    expect(repeatLabel('RRULE:FREQ=DAILY;INTERVAL=1')).toBe('每天');
    expect(repeatLabel('RRULE:FREQ=HOURLY')).toBe('自定义重复');
    expect(repeatLabel(null)).toBe('');
  });
});
