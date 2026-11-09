import { describe, expect, it } from 'vitest';
import { parseFlag, parseSet } from './ui-state';

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
