// 会话页的两个侧栏：回收站（按批次恢复 / 彻底删除）与整库备份列表。
// 从 sessions/page.tsx 拆出（主页面过长，C6 之后表格已声明式，侧栏没有理由继续挤在一起）。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { confirmAction } from '@/components/ui/confirm';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { DataTable } from '@/components/ui/table';
import { queryCmd } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { fmtSize, fmtTime } from '@/lib/format';
import { toast } from '@/components/ui/toast';
import type { Manifest, Snapshot } from '@/lib/types';

export function TrashDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const batches = useQuery<Manifest[]>({
    queryKey: ['trash'],
    queryFn: () => queryCmd('session_trash'),
    enabled: open,
  });

  const [busyTag, setBusyTag] = useState<string | null>(null);

  const restore = (tag: string) => {
    if (busyTag) return;
    setBusyTag(tag);
    void queryCmd('session_trash_restore', { tag })
      .then((r) => {
        toast(`已恢复 ${r.restored} 条${r.errors.length ? `；失败 ${r.errors.length}` : ''}`);
        // 恢复会改动回收站批次与主列表（行会重新出现），两者都要失效
        qc.invalidateQueries({ queryKey: ['trash'] });
        qc.invalidateQueries({ queryKey: ['facets'] });
        qc.invalidateQueries({ queryKey: ['sessions'] });
      })
      .catch((e) => toast(errText(e)))
      .finally(() => setBusyTag(null));
  };

  const purge = async (tag: string | null) => {
    if (busyTag) return;
    if (!(await confirmAction({ title: '确认操作', message: tag ? `彻底删除批次 ${tag}？不可恢复。` : '清空整个回收站？不可恢复。', danger: true }))) return;
    setBusyTag(tag ?? '__all__');
    void queryCmd('session_trash_purge', { tag })
      .then((bytes) => {
        toast(`已释放 ${fmtSize(bytes)}`);
        qc.invalidateQueries({ queryKey: ['trash'] });
      })
      .catch((e) => toast(errText(e)))
      .finally(() => setBusyTag(null));
  };

  return (
    <Dialog open={open} title="会话回收站" onClose={onClose} wide>
      <DataTable<Manifest>
        className="text-sm"
        dense
        columns={[
          { key: 'at', header: '删除时间', width: 150, className: 'tabular-nums', cell: (b) => fmtTime(b.at) },
          { key: 'platforms', header: '涉及客户端', className: 'text-muted', cell: (b) => b.platforms.join('、') || '—' },
          { key: 'files', header: '文件数', width: 80, align: 'right', cell: (b) => b.items.length },
          {
            key: 'size',
            header: '大小',
            width: 96,
            align: 'right',
            cell: (b) => fmtSize(b.items.reduce((sum, it) => sum + (it.size ?? 0), 0)),
          },
        ]}
        rows={batches.data ?? []}
        rowKey={(b) => b.tag}
        loading={batches.isLoading}
        skeletonRows={3}
        empty={<p className="py-6 text-center text-sm text-muted">回收站是空的。</p>}
        rowClassName={() => 'hover:bg-hover'}
        trail={{
          head: '操作',
          width: 176,
          cell: (b) => (
            <span className="flex justify-end gap-1.5">
              <Button size="sm" onClick={() => restore(b.tag)} loading={busyTag === b.tag} disabled={!!busyTag}>
                恢复
              </Button>
              <Button size="sm" variant="danger" onClick={() => purge(b.tag)} loading={busyTag === b.tag} disabled={!!busyTag}>
                彻底删除
              </Button>
            </span>
          ),
        }}
      />
      {(batches.data ?? []).length > 0 && (
        <div className="mt-3 flex items-center justify-between">
          <p className="text-xs text-muted">恢复会把文件与记录放回原处；彻底删除不可恢复。</p>
          <Button
            variant="danger"
            onClick={() => purge(null)}
            loading={busyTag === '__all__'}
            disabled={!!busyTag}
          >
            清空回收站
          </Button>
        </div>
      )}
    </Dialog>
  );
}

export function BackupsDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const backups = useQuery<Snapshot[]>({
    queryKey: ['backups'],
    queryFn: () => queryCmd('session_backups'),
    enabled: open,
  });
  // 「立即备份」放进弹窗里：工具栏只留一个「备份」入口，创建与查看在同一个地方
  const [busy, setBusy] = useState(false);
  const backup = () => {
    if (busy) return;
    setBusy(true);
    void queryCmd('session_backup')
      .then((snap) => {
        toast(`快照 ${snap.tag}：${snap.sources.length} 个来源，${fmtSize(snap.bytes)}`);
        qc.invalidateQueries({ queryKey: ['backups'] });
      })
      .catch((e) => toast(errText(e)))
      .finally(() => setBusy(false));
  };
  return (
    <Dialog
      open={open}
      title="整库备份（保留最近 10 份）"
      onClose={onClose}
      wide
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            关闭
          </Button>
          <Button variant="primary" onClick={backup} loading={busy}>
            立即备份
          </Button>
        </>
      }
    >
      <DataTable<Snapshot>
        className="text-sm"
        dense
        columns={[
          { key: 'at', header: '备份时间', width: 150, className: 'tabular-nums', cell: (s) => fmtTime(s.at) },
          {
            key: 'sources',
            header: '包含来源',
            className: 'text-muted',
            cell: (s) => (
              <>
                {s.sources.join('、')}
                {s.skipped.length > 0 && (
                  <span className="block text-xs text-muted">跳过（太大）：{s.skipped.join('、')}</span>
                )}
              </>
            ),
          },
          { key: 'bytes', header: '大小', width: 96, align: 'right', cell: (s) => fmtSize(s.bytes) },
        ]}
        rows={backups.data ?? []}
        rowKey={(s) => s.tag}
        empty={<p className="py-6 text-center text-sm text-muted">还没有备份。点「立即备份」创建第一份。</p>}
        rowClassName={() => 'hover:bg-hover'}
      />
    </Dialog>
  );
}
