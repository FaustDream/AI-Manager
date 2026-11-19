// 版本更新记录：构建期内联仓库根的 `CHANGELOG.md`，运行期不依赖任何文件。
//
// 为什么直接读 CHANGELOG：版本历史只能有一份真源。界面里再维护一份数组，
// 早晚会与提交记录对不上；这里解析 Markdown，新增版本只要写 CHANGELOG。
//
// 只解析「版本 + 日期 + 要点」三样：**不判断发布状态**——版本一旦被新版本覆盖，
// 发布状态这个概念就没有意义了，列表里也不该出现任何相关标记。
import raw from '../../CHANGELOG.md?raw';

export interface ChangeEntry {
  version: string;
  /** `YYYY-MM-DD` */
  date: string;
  /** 该版本下的要点：已去掉列表符号，缩进续行并回上一条 */
  items: string[];
}

/** `## 2.0.4（2026-11-09）`，标题尾部若还有别的字样一律忽略。 */
const HEADING = /^##\s+(\d+\.\d+\.\d+)\s*[（(]([^）)]+)[）)]/;

/**
 * 解析 CHANGELOG。只认带版本号的 `##` 分组：
 * 没有版本号的历史分组（如「早期变更」）会被跳过。
 */
export function parseChangelog(text: string): ChangeEntry[] {
  const entries: ChangeEntry[] = [];
  let current: ChangeEntry | null = null;

  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    const heading = HEADING.exec(trimmed);
    if (heading) {
      current = { version: heading[1], date: heading[2].trim(), items: [] };
      entries.push(current);
      continue;
    }
    // 任何其它二级标题都结束当前分组
    if (/^##\s+/.test(trimmed)) {
      current = null;
      continue;
    }
    if (!current) continue;

    const bullet = /^[-*]\s+(.*)$/.exec(trimmed);
    if (bullet) {
      current.items.push(bullet[1].trim());
      continue;
    }
    // 缩进续行：Markdown 里长要点会折到下一行，拼接回去才算一条完整记录
    if (current.items.length > 0 && /^\s{2,}\S/.test(line)) {
      const last = current.items.length - 1;
      current.items[last] += trimmed;
    }
  }
  return entries;
}

export const CHANGELOG: ChangeEntry[] = parseChangelog(raw);

/** 摘要里不放的行：下载产物、安装指引这类不是「这版改了什么」。 */
const NOT_A_CHANGE = /^(下载|安装|产物|校验|sha256|full changelog)/i;

/**
 * 从 GitHub 发布说明里挑最多 3 条改动要点（P1-6）。
 *
 * 发布说明是整段 Markdown（可能含多个版本分组与下载清单），直接铺进卡片会把
 * 「关于与更新」撑得比其它卡高一大截；这里只取前几条要点，全文仍走「更新说明」弹窗。
 */
export function releaseHighlights(notes: string, max = 3): string[] {
  const out: string[] = [];
  for (const line of notes.split(/\r?\n/)) {
    const bullet = /^\s*[-*]\s+(.*)$/.exec(line);
    if (!bullet) continue;
    // 去掉 Markdown 强调与行内代码标记，摘要里不需要这些符号
    const text = bullet[1].replace(/\*\*|__|`/g, '').trim();
    if (!text || NOT_A_CHANGE.test(text)) continue;
    out.push(text);
    if (out.length >= max) break;
  }
  return out;
}
