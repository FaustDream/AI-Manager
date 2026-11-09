// 设置页的「可追溯」两块：配置历史（回滚）与关于/更新。
//
// 配置写坏了没有第二次机会：定时任务一改错，自动签到就静默不跑了。
// 这里把每次写入前的旧值列出来，看得到、滚得回。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { confirmAction } from '@/components/ui/confirm';
import { Dialog } from '@/components/ui/dialog';
import { SkeletonRows } from '@/components/ui/form';
import { Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { fmtSize, fmtTime } from '@/lib/format';
import { CHANGELOG } from '@/lib/changelog';
import type { ConfigSnapshot, UpdateInfo } from '@/lib/types';

/** 配置值是 JSON 时格式化，否则原样——历史里也可能存的是纯字符串。 */
function pretty(text?: string | null): string {
  if (!text) return '（当时不存在）';
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

export function HistoryPane() {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const [viewing, setViewing] = useState<ConfigSnapshot | null>(null);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [notesOpen, setNotesOpen] = useState(false);
  const [bundlePath, setBundlePath] = useState<string | null>(null);
  const [recordsOpen, setRecordsOpen] = useState(false);
  // 快照列表收进对话框：设置页只留一行摘要，不再常驻占一大块
  const [listOpen, setListOpen] = useState(false);

  // 只用于在「更新记录」里标出哪一条是当前运行版本，不在页面上单独展示
  const version = useQuery<string>({
    queryKey: ['version'],
    queryFn: () => queryCmd('app_version'),
    staleTime: Infinity,
  });

  const history = useQuery<ConfigSnapshot[]>({
    queryKey: ['config-history'],
    queryFn: () => queryCmd('config_snapshot_list'),
  });
  const restore = (item: ConfigSnapshot) =>
    run(`restore:${item.id}`, async () => {
      const ok = await confirmAction({
        title: '回滚配置',
        message: `把「${item.key}」恢复成 ${fmtTime(item.at)} 之前的内容？\n\n当前值会先被记一条历史，滚错了还能滚回来。`,
      });
      if (!ok) return;
      if ((await runCmd('config_snapshot_restore', { id: item.id })) === undefined) return;
      toast(`已回滚 ${item.key}`);
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['config-history'] }),
        qc.invalidateQueries({ queryKey: ['schedules'] }),
      ]);
    });

  const checkUpdate = () =>
    run('update', async () => {
      const info = await runCmd('update_check');
      if (info === undefined) return;
      setUpdate(info);
      if (!info.has_update) toast('已是最新');
    });

  const exportBundle = () =>
    run('bundle', async () => {
      const path = await runCmd('diagnose_bundle');
      if (path === undefined) return;
      setBundlePath(path);
      toast(`诊断包已导出：${path}`);
    });

  return (
    <div className="flex min-w-0 flex-col gap-3">
      {/* 摘要行：详情（查看/回滚逐条）收进对话框，设置页不再被长列表撑高 */}
      <div className="rounded-lg border border-line bg-card p-4">
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="text-sm font-semibold">配置历史</h2>
          <span className="text-xs text-muted">
            {(history.data ?? []).length > 0
              ? `${(history.data ?? []).length} 条快照 · 最近「${(history.data ?? [])[0]?.key}」 ${fmtTime((history.data ?? [])[0]?.at ?? 0)}`
              : '还没有配置变更记录；回滚前会再记一条'}
          </span>
          <span className="flex-1" />
          <Button
            size="sm"
            disabled={!!busy || (history.data ?? []).length === 0}
            onClick={() => setListOpen(true)}
          >
            查看与回滚
          </Button>
          <Button
            size="sm"
            variant="danger"
            disabled={!!busy || (history.data ?? []).length === 0}
            onClick={() =>
              run('clear-history', async () => {
                if (
                  !(await confirmAction({
                    title: '清空配置历史',
                    message: '清空后无法回滚任何配置变更。确定？',
                    danger: true,
                  }))
                )
                  return;
                if ((await runCmd('config_snapshot_clear')) === undefined) return;
                toast('配置历史已清空');
                await qc.invalidateQueries({ queryKey: ['config-history'] });
              })
            }
          >
            清空
          </Button>
        </div>
      </div>

      <Dialog
        open={listOpen}
        title="配置历史"
        subtitle="最近 30 次配置变更；回滚前会再记一条"
        onClose={() => setListOpen(false)}
        wide
        footer={<Button onClick={() => setListOpen(false)}>关闭</Button>}
      >
        {history.isLoading && <SkeletonRows rows={3} cols={3} />}
        {!history.isLoading && (history.data ?? []).length === 0 && (
          <p className="text-sm text-muted">
            还没有配置变更记录：改定时任务或设置后，这里会出现可回滚的快照。
          </p>
        )}
        {(history.data ?? []).map((item) => (
          <div
            key={item.id}
            className="flex flex-wrap items-center gap-2 border-b border-line px-1 py-1.5 text-sm last:border-b-0"
          >
            <span className="tabular-nums text-muted">{fmtTime(item.at)}</span>
            <span className="font-mono text-xs">{item.key}</span>
            <Tag tone="muted">{fmtSize(item.before_bytes)}</Tag>
            <span className="flex-1" />
            <Button size="sm" variant="ghost" onClick={() => setViewing(item)}>
              查看
            </Button>
            <Button
              size="sm"
              loading={busy === `restore:${item.id}`}
              disabled={!!busy}
              onClick={() => restore(item)}
            >
              回滚
            </Button>
          </div>
        ))}
      </Dialog>

      {/* 与「配置历史」「回收站」同构：p-4 + 单行摘要，不额外加 mb，
          这样三张卡在设置页里的高度完全一致；更新结果也收进同一行。 */}
      <div className="rounded-lg border border-line bg-card p-4">
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="text-sm font-semibold">关于与更新</h2>
          {update &&
            (update.has_update ? (
              <Tag tone="warn">检测到可用更新</Tag>
            ) : (
              <Tag tone="ok">已是最新</Tag>
            ))}
          <span className="flex-1" />
          {update?.has_update && (
            <Button size="sm" variant="ghost" onClick={() => setNotesOpen(true)}>
              更新说明
            </Button>
          )}
          {update?.has_update && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                if (update) void runCmd('open_external', { url: update.url });
              }}
            >
              打开发布页
            </Button>
          )}
          <Button size="sm" onClick={() => setRecordsOpen(true)} title="逐版本查看改动要点">
            更新记录
          </Button>
          <Button size="sm" onClick={checkUpdate} loading={busy === 'update'}>
            检查更新
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={exportBundle}
            loading={busy === 'bundle'}
            title="导出日志、账号状态（脱敏）与配置，便于排查或求助"
          >
            导出诊断包
          </Button>
        </div>

        {bundlePath && (
          <div className="mt-2 flex items-center gap-2 text-xs">
            <span className="text-muted">诊断包</span>
            <span className="truncate font-mono" title={bundlePath}>
              {bundlePath}
            </span>
            <span className="flex-1" />
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                void navigator.clipboard.writeText(bundlePath);
                toast('路径已复制');
              }}
            >
              复制路径
            </Button>
          </div>
        )}
      </div>

      <Dialog
        open={viewing !== null}
        title={viewing ? `配置快照 · ${viewing.key}` : ''}
        onClose={() => setViewing(null)}
        wide
        footer={<Button onClick={() => setViewing(null)}>关闭</Button>}
      >
        {viewing && (
          <div className="grid gap-3 md:grid-cols-2">
            <div>
              <div className="mb-1 text-xs text-muted">回滚到（变更前）</div>
              <pre className="max-h-72 overflow-auto rounded-md border border-line bg-bg p-2 text-xs">
                {pretty(viewing.before)}
              </pre>
            </div>
            <div>
              <div className="mb-1 text-xs text-muted">变更后</div>
              <pre className="max-h-72 overflow-auto rounded-md border border-line bg-bg p-2 text-xs">
                {pretty(viewing.after)}
              </pre>
            </div>
          </div>
        )}
      </Dialog>

      {/* 更新记录：每个版本一条，只列版本本身的改动，不做任何发布状态判断 */}
      <Dialog
        open={recordsOpen}
        title="更新记录"
        subtitle="按版本倒序；每个版本只列改动要点"
        onClose={() => setRecordsOpen(false)}
        wide
        footer={<Button onClick={() => setRecordsOpen(false)}>关闭</Button>}
      >
        {CHANGELOG.length === 0 && <p className="text-sm text-muted">还没有版本记录。</p>}
        <div className="max-h-[60vh] overflow-y-auto">
          {CHANGELOG.map((entry) => {
            const current = version.data === entry.version;
            return (
              <div key={entry.version} className="border-b border-line px-1 py-2 last:border-b-0">
                <div className="flex flex-wrap items-baseline gap-2">
                  <span className="font-mono text-sm font-semibold">v{entry.version}</span>
                  <span className="text-xs text-muted">{entry.date}</span>
                  {current && <Tag tone="ok">当前版本</Tag>}
                </div>
                <ul className="mt-1 list-disc space-y-0.5 pl-4 text-xs text-fg2">
                  {entry.items.map((item, index) => (
                    <li key={index}>{item}</li>
                  ))}
                </ul>
              </div>
            );
          })}
        </div>
      </Dialog>

      <Dialog
        open={notesOpen}
        title="更新说明"
        onClose={() => setNotesOpen(false)}
        wide
        footer={
          <>
            <Button onClick={() => setNotesOpen(false)}>关闭</Button>
            {update && (
              <Button
                variant="primary"
                onClick={() => runCmd('open_external', { url: update.url })}
              >
                去下载
              </Button>
            )}
          </>
        }
      >
        <pre className="max-h-80 overflow-auto whitespace-pre-wrap text-xs text-fg2">
          {update?.notes || '这次发布没有写说明。'}
        </pre>
      </Dialog>
    </div>
  );
}
