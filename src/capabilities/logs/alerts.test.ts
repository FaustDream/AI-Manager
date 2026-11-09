// 弹窗触发规则：水位之前的不再弹，水位之后的才弹——这两条错了就是「反复弹」或「永不弹」。
import { describe, expect, it } from 'vitest';
import type { LogLine } from '@/lib/types';
import { ALERT_SEEN_KEY, newestAlertAt, shouldAlert } from './alerts';

function line(ts: string): LogLine {
  return {
    schema: 1,
    ts,
    level: 'error',
    module: 'test',
    message: 'boom',
    action: null,
    trigger: null,
    ok: null,
    ms: null,
    subject: null,
    step: null,
    evidence: null,
    outcome: null,
    attempts: null,
    batch: null,
    repeat: null,
    last_ts: null,
  };
}

describe('newestAlertAt', () => {
  it('取最新的一条，与顺序无关', () => {
    const rows = [
      line('2026-11-09T10:05:00+08:00'),
      line('2026-11-09T10:00:00+08:00'),
    ];
    expect(newestAlertAt(rows)).toBe(Date.parse('2026-11-09T10:05:00+08:00'));
  });

  it('空列表与非法时间都算 0，不会把水位推回过去', () => {
    expect(newestAlertAt([])).toBe(0);
    expect(newestAlertAt([line('not-a-time')])).toBe(0);
  });
});

describe('shouldAlert', () => {
  const at10 = Date.parse('2026-11-09T10:00:00+08:00');

  it('同一批错误已经提示过就不再弹', () => {
    expect(shouldAlert([line('2026-11-09T10:00:00+08:00')], at10)).toBe(false);
    expect(shouldAlert([line('2026-11-09T09:59:00+08:00')], at10)).toBe(false);
  });

  it('出现更新的错误才重新弹', () => {
    expect(shouldAlert([line('2026-11-09T11:00:00+08:00')], at10)).toBe(true);
  });

  it('没有错误时不弹', () => {
    expect(shouldAlert([], at10)).toBe(false);
    expect(shouldAlert([], 0)).toBe(false);
  });

  it('首次进入（水位 0）会弹一次', () => {
    expect(shouldAlert([line('2026-11-09T11:00:00+08:00')], 0)).toBe(true);
  });

  it('水位键名固定：改了会让所有用户的历史水位失效', () => {
    expect(ALERT_SEEN_KEY).toBe('ui.logs.alerts-seen-at');
  });
});
