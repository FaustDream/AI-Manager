import { describe, expect, it } from 'vitest';
import { numOr } from './num';

describe('numOr', () => {
  it('合法非负数字原样返回', () => {
    expect(numOr('15', 3)).toBe(15);
    expect(numOr('0', 3)).toBe(0);
  });

  it('空串 / 非法字符 / 负数回退默认值', () => {
    expect(numOr('', 3)).toBe(3);
    expect(numOr('abc', 3)).toBe(3);
    expect(numOr('-5', 3)).toBe(3);
  });

  it('小数与科学计数法是合法数字（交给上层钳制）', () => {
    expect(numOr('1.5', 3)).toBe(1.5);
  });
});
