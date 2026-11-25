// 设置页「关于」：版本更新与更新记录。
//
// 只做提示与查看：不自动下载、不替换文件，升级与否由用户决定。
import { useQuery } from '@tanstack/react-query';
import { ArrowRight } from 'lucide-react';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Section } from '@/components/ui/card';
import { Dialog } from '@/components/ui/dialog';
import { Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { CHANGELOG, releaseHighlights } from '@/lib/changelog';
import type { UpdateInfo } from '@/lib/types';

export function AboutPane() {
  const { busy, run } = useBusy();
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [notesOpen, setNotesOpen] = useState(false);
  const [recordsOpen, setRecordsOpen] = useState(false);

  // 只用于在「更新记录」里标出哪一条是当前运行版本，不在页面上单独展示
  const version = useQuery<string>({
    queryKey: ['version'],
    queryFn: () => queryCmd('app_version'),
    staleTime: Infinity,
  });

  const checkUpdate = () =>
    run('update', async () => {
      const info = await runCmd('update_check');
      if (info === undefined) return;
      setUpdate(info);
      if (!info.has_update) toast('已是最新');
    });

  return (
    <div className="flex min-w-0 flex-col gap-5">
      <Section
        title="关于与更新"
        note={
          update &&
          (update.has_update ? <Tag tone="warn">检测到可用更新</Tag> : <Tag tone="ok">已是最新</Tag>)
        }
        actions={
          <>
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
            <Button size="sm" variant="soft" onClick={checkUpdate} loading={busy === 'update'}>
              检查更新
            </Button>
          </>
        }
      >
        {/* 检查出新版就把版本号与要点摘要直接摆出来，不必再点一次「更新说明」 */}
        {update?.has_update ? (
          <div className="text-xs leading-relaxed">
            <span className="inline-flex items-center gap-1 text-muted">
              当前 {update.current}
              <ArrowRight size={12} />
              最新
            </span>{' '}
            <span className="font-semibold text-fg">{update.latest}</span>
            {update.published_at && (
              <span className="text-muted"> · 发布于 {update.published_at.slice(0, 10)}</span>
            )}
            {releaseHighlights(update.notes).length > 0 && (
              <ul className="mt-1 list-disc pl-5 text-muted">
                {releaseHighlights(update.notes).map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            )}
          </div>
        ) : update ? (
          <p className="text-xs text-muted">当前 v{update.current}</p>
        ) : null}
      </Section>

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
