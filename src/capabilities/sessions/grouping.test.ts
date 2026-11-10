// 两个列表的分组必须互不混用：同一批行分别按活跃/归档的分组器，结果标签要不同。
import { describe, expect, it } from 'vitest';
import type { SessionRow } from '@/lib/types';
import { currentScope, groupActive, groupArchived, scopeSources } from './grouping';

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

describe('scopeSources', () => {
  const facets = [
    { value: 'CodeBuddyIDE', label: 'CodeBuddy IDE', count: 8 },
    { value: 'JetBrains', label: 'CodeBuddy 插件', count: 25 },
    { value: 'qoder_plugin_cn', label: 'Qoder CN 插件', count: 1 },
    { value: 'qoder_plugin_intl', label: 'Qoder 插件', count: 1 },
    { value: 'trae_plugin_cn', label: 'Trae CN 插件', count: 1 },
    { value: 'cn', label: 'WorkBuddy 国内版', count: 2 },
  ];

  it('全部标签 = 所有源；IDEA 范围只出插件源；其他软件范围只出它自己', () => {
    expect(scopeSources(facets, 'all')).toHaveLength(6);
    expect(scopeSources(facets, 'ide-plugin').map((f) => f.value)).toEqual([
      'JetBrains',
      'qoder_plugin_cn',
      'qoder_plugin_intl',
      'trae_plugin_cn',
    ]);
    expect(scopeSources(facets, 'cn').map((f) => f.value)).toEqual(['cn']);
  });

  it('currentScope：选了插件源仍留在 IDEA 范围（聚合项不消失、可切其他插件源）', () => {
    expect(currentScope({ source: null, origin: null })).toBe('all');
    expect(currentScope({ source: null, origin: 'ide-plugin' })).toBe('ide-plugin');
    // 下拉选了插件源：source 有值、origin 已清，范围仍是 IDEA（而不是缩成单源）。
    expect(currentScope({ source: 'JetBrains', origin: null })).toBe('ide-plugin');
    expect(scopeSources(facets, currentScope({ source: 'JetBrains', origin: null }))).toHaveLength(4);
    expect(currentScope({ source: 'cn', origin: null })).toBe('cn');
  });
});

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

  it('按软件分组时国内版、国际版与 IDEA 插件各自独立', () => {
    const groups = groupActive(
      [
        row({ source_label: 'Qoder CN', source_key: 'qoder_cn' }),
        row({ source_label: 'Qoder 国际版', source_key: 'qoder_intl' }),
        row({ source_label: 'Trae CN', source_key: 'Trae CN' }),
        row({ source_label: 'Trae 国际版', source_key: 'Trae' }),
        row({ source_label: 'CodeBuddy IDE', source_key: 'CodeBuddyIDE' }),
        row({ source_label: 'IDEA 插件', source_key: 'JetBrains' }),
      ],
      'source',
    );
    expect(groups.map((group) => group.label)).toEqual([
      'Qoder CN',
      'Qoder 国际版',
      'Trae CN',
      'Trae 国际版',
      'CodeBuddy IDE',
      'IDEA 插件',
    ]);
  });

  it('各软件插件源在按软件分组里并成单一「IDEA 插件」组', () => {
    const groups = groupActive(
      [
        row({ kind: 'codebuddy', source_key: 'JetBrains', source_label: 'CodeBuddy 插件', origin: 'ide-plugin' }),
        row({ kind: 'qoder_plugin', source_key: 'qoder_plugin_cn', source_label: 'Qoder CN 插件', origin: 'ide-plugin' }),
        row({ kind: 'qoder_plugin', source_key: 'qoder_plugin_intl', source_label: 'Qoder 插件', origin: 'ide-plugin' }),
        row({ kind: 'trae_plugin', source_key: 'trae_plugin_cn', source_label: 'Trae CN 插件', origin: 'ide-plugin' }),
        row({ source_key: 'cn', source_label: 'WorkBuddy 国内版', origin: 'desktop' }),
      ],
      'source',
    );
    expect(groups.map((group) => group.label)).toEqual(['IDEA 插件', 'WorkBuddy 国内版']);
    expect(groups[0].rows).toHaveLength(4);
  });

  it('origin 标记就能进 IDEA 插件组（不依赖 source_key 名单）', () => {
    const groups = groupActive(
      [row({ source_key: 'brand_new_plugin', source_label: '新软件插件', origin: 'ide-plugin' })],
      'source',
    );
    expect(groups[0].label).toBe('IDEA 插件');
  });

  it('按账号分组时不同软件的“本机”不合并', () => {
    const groups = groupActive(
      [
        row({ source_label: 'Qoder CN', account: '', account_label: '本机' }),
        row({ source_label: 'Trae CN', account: '', account_label: '本机' }),
      ],
      'account',
    );
    expect(groups.map((group) => group.label)).toEqual([
      'Qoder CN · 本机',
      'Trae CN · 本机',
    ]);
  });

  it('缺字段时给可读的兜底标签', () => {
    const bare = [row({ project: '', account: '', account_label: '' })];
    expect(groupActive(bare, 'project')[0].label).toBe('未标注项目');
    expect(groupActive(bare, 'account')[0].label).toBe('WorkBuddy 国内版 · 本机');
    expect(groupActive(bare, 'source')[0].label).toBe('WorkBuddy 国内版');
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

  it('归档列表同样按软件拆到 IDEA 插件，插件源不再各成组', () => {
    const groups = groupArchived(
      [
        row({ session_id: 'c', source_label: 'CodeBuddy IDE', source_key: 'CodeBuddyIDE' }),
        row({ session_id: 'd', source_label: 'CodeBuddy 插件', source_key: 'JetBrains', origin: 'ide-plugin' }),
        row({ session_id: 'e', source_label: 'Qoder 插件', source_key: 'qoder_plugin_intl', origin: 'ide-plugin' }),
      ],
      'source',
    );
    expect(groups.map((group) => group.label)).toEqual(['CodeBuddy IDE', 'IDEA 插件']);
    expect(groups[1].rows).toHaveLength(2);
  });
});
