import { describe, expect, it } from 'vitest';
import { FONT_CHOICES, loadFontChoice, parseChoice, parseFlag, parseSet } from './ui-state';

describe('parseFlag', () => {
  it('缺省时用回退值', () => {
    expect(parseFlag(null, true)).toBe(true);
    expect(parseFlag(null, false)).toBe(false);
  });

  it('只认 1/true，其余按关闭处理', () => {
    expect(parseFlag('1', false)).toBe(true);
    expect(parseFlag('true', false)).toBe(true);
    expect(parseFlag('0', true)).toBe(false);
    expect(parseFlag('随便写的', true)).toBe(false);
  });
});

describe('parseChoice', () => {
  const values = ['none', 'source', 'account'] as const;

  it('缺省或脏值时回退默认值', () => {
    expect(parseChoice(null, 'source', values)).toBe('source');
    expect(parseChoice('旧版本的值', 'source', values)).toBe('source');
  });

  it('只接受白名单里的值', () => {
    expect(parseChoice('account', 'source', values)).toBe('account');
  });
});

// 字号档位（P1-13）：纯显示偏好，脏值绝不能把界面卡在一个不存在的档位上。
describe('字号档位', () => {
  it('只有标准与大两档', () => {
    expect([...FONT_CHOICES]).toEqual(['standard', 'large']);
  });

  it('白名单外的值回退标准', () => {
    expect(parseChoice('huge', 'standard', FONT_CHOICES)).toBe('standard');
    expect(parseChoice(null, 'standard', FONT_CHOICES)).toBe('standard');
    expect(parseChoice('large', 'standard', FONT_CHOICES)).toBe('large');
  });

  it('node 环境没有 localStorage 也不抛错，回退标准', () => {
    expect(loadFontChoice()).toBe('standard');
  });
});

describe('parseSet', () => {
  it('解析字符串数组', () => {
    expect([...parseSet('["a","b"]')]).toEqual(['a', 'b']);
  });

  it('损坏或非数组时给空集，不抛错', () => {
    expect(parseSet(null).size).toBe(0);
    expect(parseSet('不是 JSON').size).toBe(0);
    expect(parseSet('{"a":1}').size).toBe(0);
  });

  it('丢弃非字符串项', () => {
    expect([...parseSet('["a",1,null,{"b":2}]')]).toEqual(['a']);
  });
});
