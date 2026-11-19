import { describe, expect, it } from 'vitest';
import { parseChangelog, releaseHighlights } from './changelog';

const SAMPLE = `# 更新记录

人工维护的变更摘要。

## 2.0.4（2026-11-09）

只呈现版本本身。

- 变更：更新记录不再区分发布状态，
  也不展示任何标记
- 变更：历史版本只列改动要点

## 2.0.0（2026-11-07）

正式版。

- 修复：动作日志触发语义

## 早期变更（未编号）

没有版本号的历史分组，应被忽略。

- 这条不该出现在结果里
`;

describe('parseChangelog', () => {
  it('按版本号切分并保留日期与要点', () => {
    const entries = parseChangelog(SAMPLE);
    expect(entries.map((entry) => entry.version)).toEqual(['2.0.4', '2.0.0']);
    expect(entries[1].date).toBe('2026-11-07');
    expect(entries[0].items).toHaveLength(2);
  });

  it('缩进续行会并回上一条要点', () => {
    const [latest] = parseChangelog(SAMPLE);
    expect(latest.items[0]).toBe('变更：更新记录不再区分发布状态，也不展示任何标记');
  });

  it('无版本号的分组被跳过，条目里不带发布状态字段', () => {
    const entries = parseChangelog(SAMPLE);
    expect(entries.some((entry) => entry.items.some((item) => item.includes('不该出现')))).toBe(
      false,
    );
    expect(Object.keys(entries[0]).sort()).toEqual(['date', 'items', 'version']);
  });

  it('标题尾部的额外字样不会混进日期', () => {
    const [entry] = parseChangelog('## 2.0.5（2026-11-10）· 随便写的\n\n- 一条要点\n');
    expect(entry.date).toBe('2026-11-10');
    expect(entry.version).toBe('2.0.5');
  });
});

describe('releaseHighlights（发布说明摘要）', () => {
  it('只取要点行，去掉列表符号与 Markdown 强调', () => {
    const notes = '## 2.0.9\n\n一段说明文字。\n\n- **修复**：签到失败归因\n* 新增：字号档位\n';
    expect(releaseHighlights(notes)).toEqual(['修复：签到失败归因', '新增：字号档位']);
  });

  it('最多给 3 条：摘要不是全文，长了就把卡片撑爆', () => {
    const notes = ['- 一', '- 二', '- 三', '- 四', '- 五'].join('\n');
    expect(releaseHighlights(notes)).toEqual(['一', '二', '三']);
  });

  it('没有要点行时给空数组，而不是把整段说明塞进来', () => {
    expect(releaseHighlights('## 2.0.9\n\n只有散文，没有列表。\n')).toEqual([]);
    expect(releaseHighlights('')).toEqual([]);
  });

  it('跳过下载链接与安装提示这类非改动行', () => {
    const notes = [
      '- 修复：通知授权链路',
      '- 下载：AI-Manager-2.0.9-windows-x64-setup.exe',
      '- 安装说明见 README',
    ].join('\n');
    expect(releaseHighlights(notes)).toEqual(['修复：通知授权链路']);
  });
});
