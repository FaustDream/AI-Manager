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

/** 活跃列表分组：项目 / 账号 / 状态。 */
export type ActiveGroup = 'none' | 'project' | 'account' | 'state';

export function groupActive(rows: SessionRow[], mode: ActiveGroup): RowGroup[] {
  if (mode === 'none') return [{ key: 'all', label: '', rows }];
  const out: RowGroup[] = [];
  const index = new Map<string, number>();
  for (const row of rows) {
    const label =
      mode === 'project'
        ? row.project || '未标注项目'
        : mode === 'account'
          ? row.account_label || '本机'
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
        ? row.source_label || '未知软件'
        : mode === 'project'
          ? row.project || '未标注项目'
          : row.account_label || '本机';
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
