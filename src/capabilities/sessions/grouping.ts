// 两个会话列表的分组器。**刻意分成两套**：活跃列表按项目/账号/状态，
// 归档列表按软件/项目/账号——入口、选项与语义都不共用，避免两个列表互相影响。
import type { SessionRow } from '@/lib/types';

export const STATE_LABELS: Record<string, string> = {
  visible: '可见',
  deleted: '已删除',
  old_account: '旧账号',
  cloud: '云端',
  archived: '已归档',
  orphan: '孤立',
};

export interface RowGroup {
  key: string;
  label: string;
  rows: SessionRow[];
}

/** 活跃列表分组：软件 / 项目 / 账号 / 状态。 */
export type ActiveGroup = 'none' | 'source' | 'project' | 'account' | 'state';

/** 账号分组不能把不同软件的“本机”并在一起。 */
function accountKey(row: SessionRow): string {
  const label = row.account_label?.trim();
  if (label && label !== '本机') return label;
  return `${row.source_label || '未知软件'} · 本机`;
}

/** 被「IDEA 插件」标签聚合的插件源（与来源标签栏同一集合）：按软件分组时并成一组，
 *  不再各软件插件单独成组；桌面端来源不受影响。 */
const PLUGIN_SOURCE_KEYS = new Set(['JetBrains', 'qoder_plugin_cn', 'qoder_plugin_intl', 'trae_plugin_cn']);

export function isPluginSource(row: SessionRow): boolean {
  return PLUGIN_SOURCE_KEYS.has(row.source_key) || row.origin === 'ide-plugin';
}

/** facet 里「来源」项：与生成的 FacetItem 结构兼容，渲染需要 label/count。 */
export interface SourceFacet {
  value: string;
  label: string;
  count: number;
}

/** 来源标签栏面向 facet（只有 value/label）：按来源键判定是否插件源。 */
export function isPluginFacet(facet: SourceFacet): boolean {
  return PLUGIN_SOURCE_KEYS.has(facet.value);
}

/** 当前来源标签：全部 / IDEA 插件聚合 / 某个具体源。 */
export type SourceScope = 'all' | 'ide-plugin' | string;

/** 「来源」下拉的选项范围只跟着**标签**走，不跟着下拉自己的选中值走
 *  （否则选了插件源后聚合项消失、也换不了同组其他源）：
 *  全部 = 所有源；IDEA 插件 = 只出插件源；其他软件 = 只有它自己。 */
export function scopeSources(
  facets: SourceFacet[],
  scope: SourceScope,
): SourceFacet[] {
  if (scope === 'ide-plugin') return facets.filter(isPluginFacet);
  if (scope !== 'all') return facets.filter((f) => f.value === scope);
  return facets;
}

/** 来源键是否属于插件源（标签高亮用：下拉选了插件源时 IDEA 标签仍算选中态）。 */
export function isPluginKey(sourceKey: string | null): boolean {
  return sourceKey !== null && PLUGIN_SOURCE_KEYS.has(sourceKey);
}

/** 从当前筛选反推处于哪个标签：下拉选了插件源（source 有值且是插件）仍算 IDEA 标签。 */
export function currentScope(filter: { source: string | null; origin: string | null }): SourceScope {
  if (filter.origin === 'ide-plugin') return 'ide-plugin';
  if (filter.source === null) return 'all';
  return PLUGIN_SOURCE_KEYS.has(filter.source) ? 'ide-plugin' : filter.source;
}

export function sourceLabel(row: SessionRow): string {
  return isPluginSource(row) ? 'IDEA 插件' : row.source_label || '未知软件';
}

export function groupActive(rows: SessionRow[], mode: ActiveGroup): RowGroup[] {
  if (mode === 'none') return [{ key: 'all', label: '', rows }];
  const out: RowGroup[] = [];
  const index = new Map<string, number>();
  for (const row of rows) {
    const label =
      mode === 'source'
        ? sourceLabel(row)
        : mode === 'project'
          ? row.project || '未标注项目'
          : mode === 'account'
            ? accountKey(row)
            : STATE_LABELS[row.state] ?? row.state;
    let at = index.get(label);
    if (at === undefined) {
      at = out.length;
      index.set(label, at);
      out.push({ key: `${mode}:${label}`, label, rows: [] });
    }
    out[at].rows.push(row);
  }
  return out;
}

/** 归档列表分组：软件 / 项目 / 账号。 */
export type ArchiveGroup = 'none' | 'source' | 'project' | 'account';

export function groupArchived(rows: SessionRow[], mode: ArchiveGroup): RowGroup[] {
  if (mode === 'none') return [{ key: 'all', label: '', rows }];
  const out: RowGroup[] = [];
  const index = new Map<string, number>();
  for (const row of rows) {
    const label =
      mode === 'source'
        ? sourceLabel(row)
        : mode === 'project'
          ? row.project || '未标注项目'
          : accountKey(row);
    let at = index.get(label);
    if (at === undefined) {
      at = out.length;
      index.set(label, at);
      out.push({ key: `archive:${mode}:${label}`, label, rows: [] });
    }
    out[at].rows.push(row);
  }
  return out;
}
