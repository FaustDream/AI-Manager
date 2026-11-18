// 会话列表的分组器：平台 / 项目 / 状态。
//
// 列表只有一份（所有平台、所有账号的会话合在一起），分组只影响「怎么摆」，
// 不影响「有哪些」——筛选是下拉的事。活跃与归档共用同一个分组器：
// 归档列表整表都是归档件，界面上不提供「按状态」这一档而已。
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

/** 分组维度。key 与列表列名一一对应（分组时把那一列藏掉，信息不重复）。 */
export type GroupMode = 'none' | 'source' | 'project' | 'state';

export function groupRows(rows: SessionRow[], mode: GroupMode): RowGroup[] {
  if (mode === 'none') return [{ key: 'all', label: '', rows }];
  const out: RowGroup[] = [];
  const index = new Map<string, number>();
  for (const row of rows) {
    const label =
      mode === 'source'
        ? row.source_label || '未知平台'
        : mode === 'project'
          ? row.project || '未标注项目'
          : (STATE_LABELS[row.state] ?? row.state);
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
