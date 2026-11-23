import { describe, expect, it } from 'vitest';
import { matchesQuery, nextSort, sortRows, type SortState } from './sort';

describe('nextSort：三态循环', () => {
  it('没排序时点一列就是升序', () => {
    expect(nextSort(null, 'balance')).toEqual<SortState>({ key: 'balance', dir: 'asc' });
  });

  it('同一列连点：升 → 降 → 取消', () => {
    const asc = nextSort(null, 'balance');
    const desc = nextSort(asc, 'balance');
    expect(desc).toEqual<SortState>({ key: 'balance', dir: 'desc' });
    expect(nextSort(desc, 'balance')).toBeNull();
  });

  it('换一列重置成升序，不继承上一次方向', () => {
    const desc: SortState = { key: 'balance', dir: 'desc' };
    expect(nextSort(desc, 'name')).toEqual<SortState>({ key: 'name', dir: 'asc' });
  });
});

describe('sortRows', () => {
  const rows = [
    { name: '账号 10', n: 10 },
    { name: '账号 2', n: 2 },
    { name: '没有值', n: null },
  ];
  const of = (row: (typeof rows)[number], key: string) => (key === 'n' ? row.n : row.name);

  it('数字升序 / 降序', () => {
    expect(sortRows(rows, { key: 'n', dir: 'asc' }, of).map((r) => r.n)).toEqual([2, 10, null]);
    expect(sortRows(rows, { key: 'n', dir: 'desc' }, of).map((r) => r.n)).toEqual([10, 2, null]);
  });

  it('空值升序降序都沉底', () => {
    const up = sortRows(rows, { key: 'n', dir: 'asc' }, of);
    const down = sortRows(rows, { key: 'n', dir: 'desc' }, of);
    expect(up[up.length - 1].n).toBeNull();
    expect(down[down.length - 1].n).toBeNull();
  });

  it('中文按数字感知比较：账号 2 在账号 10 前面', () => {
    const sorted = sortRows(rows, { key: 'name', dir: 'asc' }, of);
    expect(sorted.map((r) => r.name)).toEqual(['没有值', '账号 2', '账号 10']);
  });

  it('不传排序时原样返回，不改入参数组', () => {
    const before = [...rows];
    expect(sortRows(rows, null, of)).toBe(rows);
    expect(rows).toEqual(before);
  });

  it('稳定：相等值保持原有相对顺序', () => {
    const same = [
      { id: 'a', n: 1 },
      { id: 'b', n: 1 },
      { id: 'c', n: 1 },
    ];
    const sorted = sortRows(same, { key: 'n', dir: 'desc' }, (row) => row.n);
    expect(sorted.map((r) => r.id)).toEqual(['a', 'b', 'c']);
  });
});

describe('matchesQuery', () => {
  const hay = ['主号', 'WorkBuddy 国内版', 'workbuddy:u1', '绑定在台式机'];

  it('空关键词一律通过', () => {
    expect(matchesQuery(hay, '')).toBe(true);
    expect(matchesQuery(hay, '   ')).toBe(true);
  });

  it('多关键词按 AND 匹配，跨字段也算命中', () => {
    expect(matchesQuery(hay, 'work 台式')).toBe(true);
    expect(matchesQuery(hay, 'work 笔记本')).toBe(false);
  });

  it('不区分大小写，全角空格也能分词', () => {
    expect(matchesQuery(hay, 'WORKBUDDY')).toBe(true);
    expect(matchesQuery(hay, '主号　台式')).toBe(true);
  });
});
