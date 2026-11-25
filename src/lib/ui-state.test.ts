import { describe, expect, it } from 'vitest';
import {
  FONT_CHOICES,
  loadFontChoice,
  orderItems,
  parseChoice,
  parseFlag,
  parseOrder,
  parseSet,
  reorderIds,
  withPageEnabled,
} from './ui-state';

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

// 侧边栏拖拽排序：存档只记 id 次序，新增/下线菜单项都不该让顺序失效。
describe('菜单顺序', () => {
  const items = [{ id: 'a' }, { id: 'b' }, { id: 'c' }, { id: 'd' }];
  const ids = (list: { id: string }[]) => list.map((item) => item.id);

  it('按存档顺序重排，未记录的新项保持原有相对次序追加在末尾', () => {
    expect(ids(orderItems(items, (item) => item.id, ['c', 'a']))).toEqual(['c', 'a', 'b', 'd']);
  });

  it('忽略存档里已不存在的 id，重复项只认第一次', () => {
    expect(ids(orderItems(items, (item) => item.id, ['x', 'b', 'a', 'b']))).toEqual(['b', 'a', 'c', 'd']);
  });

  it('空存档保持默认顺序', () => {
    expect(ids(orderItems(items, (item) => item.id, []))).toEqual(['a', 'b', 'c', 'd']);
  });

  it('parseOrder 只收字符串项，损坏时给空数组', () => {
    expect(parseOrder('["a",1,null,"b"]')).toEqual(['a', 'b']);
    expect(parseOrder(null)).toEqual([]);
    expect(parseOrder('不是 JSON')).toEqual([]);
    expect(parseOrder('{"a":1}')).toEqual([]);
  });
});

// 页面开关：存「关掉的 id」，新上线的页面缺省可见，不需要迁移存档。
describe('页面开关', () => {
  it('关闭加入集合、打开移除集合', () => {
    const hidden = new Set<string>();
    const off = withPageEnabled(hidden, 'logs', false);
    expect([...off]).toEqual(['logs']);
    expect([...withPageEnabled(off, 'logs', true)]).toEqual([]);
  });

  it('返回新集合，不改动传入的快照', () => {
    const hidden = new Set(['logs']);
    const next = withPageEnabled(hidden, 'hub', false);
    expect([...hidden]).toEqual(['logs']);
    expect([...next]).toEqual(['logs', 'hub']);
  });
});

describe('reorderIds', () => {
  const ids = ['a', 'b', 'c', 'd'];

  it('放到目标项之前 / 之后', () => {
    expect(reorderIds(ids, 'a', 'c', true)).toEqual(['b', 'a', 'c', 'd']);
    expect(reorderIds(ids, 'a', 'c', false)).toEqual(['b', 'c', 'a', 'd']);
  });

  it('往反方向拖同样按「移除源项之后」的落点计算', () => {
    expect(reorderIds(ids, 'd', 'b', true)).toEqual(['a', 'd', 'b', 'c']);
    expect(reorderIds(ids, 'd', 'b', false)).toEqual(['a', 'b', 'd', 'c']);
  });

  it('源就是目标、或目标不在列表里时不改顺序', () => {
    expect(reorderIds(ids, 'b', 'b', true)).toBeNull();
    expect(reorderIds(ids, 'b', 'x', true)).toBeNull();
  });
});
