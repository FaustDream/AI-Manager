// 两个列表的分组必须互不混用：同一批行分别按活跃/归档的分组器，结果标签要不同。
import { describe, expect, it } from 'vitest';
import type { SessionRow } from '@/lib/types';
import { groupActive, groupArchived } from './grouping';

const row = (over: Partial<SessionRow>): SessionRow =>
  ({
    kind: 'workbuddy',
    source_key: 'cn',
    source_label: 'WorkBuddy 国内版',
    account: 'u1',
    account_label: '凌致',
    session_id: 's1',
    title: 't',
    last_at: null,
    msgs: 1,
    size: 1,
    state: 'visible',
    has_body: true,
    project: 'AI Manager',
    cwd: 'D:/AI Manager',
    ...over,
  }) as SessionRow;

describe('groupActive', () => {
  const rows = [
    row({ session_id: 'a', project: 'AI Manager', account_label: '凌致', source_label: 'WorkBuddy 国内版' }),
    row({ session_id: 'b', project: 'AI Manager', account_label: '凌致', source_label: 'WorkBuddy 国内版' }),
    row({ session_id: 'c', project: '其它', account_label: 'Faust', source_label: 'Qoder CN', state: 'cloud' }),
  ];

  it('不分组时就是一整组', () => {
    expect(groupActive(rows, 'none')).toHaveLength(1);
  });

  it('按项目 / 账号 / 状态都能正确聚合', () => {
    const byProject = groupActive(rows, 'project');
    expect(byProject.map((g) => g.label)).toEqual(['AI Manager', '其它']);
    expect(byProject[0].rows).toHaveLength(2);

    const byAccount = groupActive(rows, 'account');
    expect(byAccount.map((g) => g.label)).toEqual(['凌致', 'Faust']);

    const byState = groupActive(rows, 'state');
    expect(byState.map((g) => g.label)).toEqual(['可见', '云端']);
  });

  it('缺字段时给可读的兜底标签', () => {
    const bare = [row({ project: '', account_label: '' })];
    expect(groupActive(bare, 'project')[0].label).toBe('未标注项目');
    expect(groupActive(bare, 'account')[0].label).toBe('本机');
  });
});

describe('groupArchived', () => {
  const rows = [
    row({ session_id: 'a', state: 'archived', source_label: 'WorkBuddy 国内版', project: 'AI Manager' }),
    row({ session_id: 'b', state: 'archived', source_label: 'Qoder CN', project: 'AI Manager' }),
  ];

  it('按软件分组与活跃列表的项目分组不是同一套', () => {
    expect(groupArchived(rows, 'source').map((g) => g.label)).toEqual([
      'WorkBuddy 国内版',
      'Qoder CN',
    ]);
    // 同一批数据按项目只会有一组，说明两个分组器互不串台
    expect(groupArchived(rows, 'project')).toHaveLength(1);
  });

  it('按项目 / 账号分组可用', () => {
    expect(groupArchived(rows, 'project')[0].rows).toHaveLength(2);
    expect(groupArchived(rows, 'account').map((g) => g.label)).toEqual(['凌致']);
  });
});
