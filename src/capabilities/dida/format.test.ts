import { describe, expect, it } from 'vitest';
import { formatDue, fromDidaDue, isCompleted, priorityLabel, toDidaDue } from './format';

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
