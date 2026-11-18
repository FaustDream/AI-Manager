// 分组器：一份列表按平台 / 项目 / 状态聚合，同一批行的标签与分组数必须稳定。
import { describe, expect, it } from 'vitest';
import type { SessionRow } from '@/lib/types';
import { groupRows } from './grouping';

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
    pinned: false,
    renamed: false,
    snippet: null,
    ...over,
  }) as SessionRow;

describe('groupRows', () => {
  const rows = [
    row({ session_id: 'a', project: 'AI Manager', source_label: 'WorkBuddy 国内版' }),
    row({ session_id: 'b', project: 'AI Manager', source_label: 'WorkBuddy 国内版' }),
    row({ session_id: 'c', project: '其它', source_label: 'Qoder CN', state: 'cloud' }),
  ];

  it('不分组时就是一整组', () => {
    expect(groupRows(rows, 'none')).toHaveLength(1);
    expect(groupRows(rows, 'none')[0].rows).toHaveLength(3);
  });

  it('按平台 / 项目 / 状态都能正确聚合', () => {
    expect(groupRows(rows, 'source').map((g) => g.label)).toEqual([
      'WorkBuddy 国内版',
      'Qoder CN',
    ]);

    const byProject = groupRows(rows, 'project');
    expect(byProject.map((g) => g.label)).toEqual(['AI Manager', '其它']);
    expect(byProject[0].rows).toHaveLength(2);

    const byState = groupRows(rows, 'state');
    expect(byState.map((g) => g.label)).toEqual(['可见', '云端']);
  });

  it('各平台独立成组，不再聚合成「IDEA 插件」', () => {
    const groups = groupRows(
      [
        row({ source_label: 'Qoder CN', source_key: 'qoder_cn' }),
        row({ source_label: 'Qoder 国际版', source_key: 'qoder_intl' }),
        row({ source_label: 'Trae CN', source_key: 'trae_cn' }),
        row({ source_label: 'Trae 国际版', source_key: 'trae' }),
        row({ source_label: 'CodeBuddy IDE', source_key: 'CodeBuddyIDE' }),
        row({ source_label: 'CodeBuddy 插件', source_key: 'JetBrains', origin: 'ide-plugin' }),
        row({ source_label: 'Qoder CN 插件', source_key: 'qoder_plugin_cn', origin: 'ide-plugin' }),
      ],
      'source',
    );
    expect(groups.map((group) => group.label)).toEqual([
      'Qoder CN',
      'Qoder 国际版',
      'Trae CN',
      'Trae 国际版',
      'CodeBuddy IDE',
      'CodeBuddy 插件',
      'Qoder CN 插件',
    ]);
  });

  it('缺字段时给可读的兜底标签', () => {
    const bare = [row({ project: '', source_label: '' })];
    expect(groupRows(bare, 'project')[0].label).toBe('未标注项目');
    expect(groupRows(bare, 'source')[0].label).toBe('未知平台');
  });

  it('分组键带维度前缀，两个维度不会撞 key', () => {
    const keys = new Set([
      ...groupRows(rows, 'source').map((g) => g.key),
      ...groupRows(rows, 'project').map((g) => g.key),
      ...groupRows(rows, 'state').map((g) => g.key),
    ]);
    expect(keys.size).toBe(2 + 2 + 2);
  });
});
