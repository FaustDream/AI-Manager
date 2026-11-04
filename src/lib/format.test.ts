// 前端第一条测试：时间/数值格式化的纯函数契约。
// 「按本地时区显示」这条约定原先挂在 Rust 契约 18 上，但它测的是 chrono 自身；
// 真正产出展示文本的是这里，所以契约落在这一层（TZ 由 vitest.config.ts 固定）。
import { describe, expect, it } from 'vitest';
import { fmtAmount, fmtDelta, fmtSize, fmtTime } from './format';

describe('fmtTime', () => {
  it('按本地时区渲染为 MM-DD HH:mm（TZ 固定 Asia/Shanghai）', () => {
    // 2026-10-30T14:00:00Z + 08:00 = 22:00
    expect(fmtTime(Date.UTC(2026, 9, 30, 14, 0, 0))).toBe('10-30 22:00');
    expect(fmtTime('2026-10-30T14:00:00Z')).toBe('10-30 22:00');
  });

  it('跨日按本地日期取值', () => {
    // 2026-10-30T18:00:00Z + 08:00 = 10-31 02:00
    expect(fmtTime(Date.UTC(2026, 9, 30, 18, 0, 0))).toBe('10-31 02:00');
  });

  it('空值与非法输入显示占位符', () => {
    expect(fmtTime(null)).toBe('—');
    expect(fmtTime(undefined)).toBe('—');
    expect(fmtTime('')).toBe('—');
    expect(fmtTime('not-a-date')).toBe('—');
  });
});

describe('fmtSize / fmtAmount / fmtDelta', () => {
  it('大小按 1024 进制换单位', () => {
    expect(fmtSize(0)).toBe('0 B');
    expect(fmtSize(2048)).toBe('2.0 KB');
    expect(fmtSize(5 * 1024 * 1024)).toBe('5.0 MB');
    expect(fmtSize(null)).toBe('—');
  });

  it('金额过万/过亿换中文单位', () => {
    expect(fmtAmount(12.5)).toBe('12.50');
    expect(fmtAmount(12_000)).toBe('1.20万');
    expect(fmtAmount(120_000_000)).toBe('1.20亿');
    expect(fmtAmount(null)).toBe('—');
  });

  it('增量带正号', () => {
    expect(fmtDelta(3)).toBe('+3');
    expect(fmtDelta(-3)).toBe('-3');
    expect(fmtDelta(null)).toBe('—');
  });
});
