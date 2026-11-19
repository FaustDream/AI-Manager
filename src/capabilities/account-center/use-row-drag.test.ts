// 排序的纯算法：拖拽落点与键盘上移/下移共用同一套语义，
// 差别只在「插入位置怎么来」。这里先把它测住，再让两条入口都用它。
import { describe, expect, it } from 'vitest';
import { moveBy, insertAt } from './use-row-drag';

describe('moveBy（键盘上移/下移）', () => {
  const ids = ['a', 'b', 'c', 'd'];

  it('上移一位与相邻元素交换', () => {
    expect(moveBy(ids, 'c', -1)).toEqual(['a', 'c', 'b', 'd']);
  });

  it('下移一位与相邻元素交换', () => {
    expect(moveBy(ids, 'b', 1)).toEqual(['a', 'c', 'b', 'd']);
  });

  it('已在首/尾时返回 null，不产生无意义的写盘', () => {
    expect(moveBy(ids, 'a', -1)).toBeNull();
    expect(moveBy(ids, 'd', 1)).toBeNull();
  });

  it('不认识的 id 返回 null', () => {
    expect(moveBy(ids, 'zzz', 1)).toBeNull();
  });

  it('不改动原数组', () => {
    const before = [...ids];
    moveBy(ids, 'a', 1);
    expect(ids).toEqual(before);
  });

  it('单元素列表两个方向都无处可去', () => {
    expect(moveBy(['a'], 'a', -1)).toBeNull();
    expect(moveBy(['a'], 'a', 1)).toBeNull();
  });
});

describe('insertAt（拖拽落点）', () => {
  const ids = ['a', 'b', 'c', 'd'];

  it('落点语义 = 移除源行后的插入位置', () => {
    // 把 d 拖到 b 之前 → a d b c
    expect(insertAt(ids, 'd', 'b', true)).toEqual(['a', 'd', 'b', 'c']);
    // 把 a 拖到 c 之后 → b c a d
    expect(insertAt(ids, 'a', 'c', false)).toEqual(['b', 'c', 'a', 'd']);
  });

  it('拖到自己身上返回 null（原地点一下不算排序）', () => {
    expect(insertAt(ids, 'b', 'b', true)).toBeNull();
    expect(insertAt(ids, 'b', 'b', false)).toBeNull();
  });

  it('落点不存在时返回 null', () => {
    expect(insertAt(ids, 'a', 'zzz', true)).toBeNull();
  });
});