// 设置中心：通用（主题/开机自启）与维护（空间/回收站/打开目录/v1 归档）。
// 日志与任务是独立能力（capabilities/logs、capabilities/tasks），不在这里。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy, errMsg } from '@/lib/cmd';
import { useEffect, useState } from 'react';

import { fmtSize } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Checkbox, Select, SkeletonRows } from '@/components/ui/form';
import { Notice } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { saveTheme, type ThemeChoice } from '@/lib/theme';
import type { CountPair, SpaceEntry, SpaceReport } from '@/lib/types';

// 设置只有一屏：左边「通用」（窄，就两个开关），右边「维护」（宽，表格多）。
// 不分成两个页签——一页一次只显示一半，窗口大半是空的。
export default function SettingsPage() {
  return (
    <div className="grid items-start gap-3 lg:grid-cols-[320px_1fr]">
      <GeneralPane />
      <MaintenancePane />
    </div>
  );
}

function GeneralPane() {
  const [theme, setTheme] = useState<ThemeChoice>('');
  const [autostart, setAutostart] = useState<boolean | null>(null);

  useEffect(() => {
    queryCmd('settings_get', { key: 'ui.theme' })
      .then((v) => setTheme((v as ThemeChoice) || ''))
      .catch(() => {});
    queryCmd('autostart_get')
      .then(setAutostart)
      .catch(() => setAutostart(false));
  }, []);

  return (
    <div className="flex flex-col gap-4 rounded-lg border border-line bg-card p-4">
      <label className="flex items-center gap-4 text-sm">
        <span className="w-24 text-muted">主题</span>
        <Select
          value={theme}
          onChange={async (e) => {
            const value = e.target.value as ThemeChoice;
            setTheme(value);
            await saveTheme(value);
          }}
        >
          <option value="">跟随系统</option>
          <option value="light">浅色</option>
          <option value="dark">深色</option>
          <option value="classic-light">经典浅色（暖纸）</option>
          <option value="classic-dark">经典深色（暖黑）</option>
        </Select>
      </label>
      <p className="-mt-2 text-xs text-muted">
        「经典」两套是暖色调：米纸底配棕强调（浅）、暖黑底配琥珀强调（深）；
        默认两套是冷灰蓝。切换即时生效并持久化。
      </p>
      <label className="flex items-center gap-4 text-sm">
        <span className="w-24 text-muted">开机自启</span>
        <Checkbox
          checked={autostart ?? false}
          disabled={autostart === null}
          onChange={async (e) => {
            const enabled = e.target.checked;
            setAutostart(enabled);
            const done = await runCmd('autostart_set', { enabled }, {
              onError: (err) => {
                setAutostart(!enabled);
                toast(errMsg(err));
              },
            });
            if (done !== undefined)
              toast(enabled ? '已开机自启（写 HKCU Run）' : '已关闭开机自启');
          }}
        />
      </label>
      <p className="text-xs text-muted">
        日志级别在「日志」页调整；数据目录在「维护」页打开。
      </p>
    </div>
  );
}

function MaintenancePane() {
  const qc = useQueryClient();
  const { busy, run: runBusy } = useBusy();
  const space = useQuery<SpaceReport>({
    queryKey: ['space'],
    queryFn: () => queryCmd('maintenance_space'),
  });
  const trash = useQuery<SpaceEntry[]>({
    queryKey: ['maint-trash'],
    queryFn: () => queryCmd('maintenance_trash'),
  });
  const [verify, setVerify] = useState<string | null>(null);

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="rounded-lg border border-line bg-card p-4">
        <div className="mb-2 flex items-center gap-2">
          <h2 className="text-sm font-semibold">数据目录占用</h2>
          <span className="flex-1" />
          <Button
            size="sm"
            onClick={async () => {
              
                if ((await runCmd('maintenance_open_data')) === undefined) return;
              
            }}
          >
            打开数据目录
          </Button>
        </div>
        {space.isLoading && <SkeletonRows rows={4} cols={2} />}
        {(space.data?.entries ?? []).map((entry) => (
          <div key={entry.name} className="flex items-center gap-2 border-b border-line px-1 py-1 text-sm last:border-b-0">
            <span className="flex-1 truncate" title={entry.path}>{entry.name}</span>
            <span className="tabular-nums text-muted">{fmtSize(entry.size)}</span>
          </div>
        ))}
        {space.data && <div className="mt-1 text-xs text-muted">合计 {fmtSize(space.data.total)}</div>}
      </div>

      <div className="rounded-lg border border-line bg-card p-4">
        <div className="mb-2 flex items-center gap-2">
          <h2 className="text-sm font-semibold">回收站（会话删除与平台技能移除）</h2>
          <span className="flex-1" />
          <Button size="sm" variant="ghost" onClick={() => qc.invalidateQueries({ queryKey: ['maint-trash'] })}>
            刷新
          </Button>
        </div>
        {(trash.data ?? []).length === 0 && <p className="text-sm text-muted">空的。</p>}
        {(trash.data ?? []).map((batch) => (
          <div key={batch.path} className="flex items-center gap-2 border-b border-line px-1 py-1.5 text-sm last:border-b-0">
            <span className="flex-1 truncate font-mono text-xs" title={batch.path}>{batch.name}</span>
            <span className="tabular-nums text-muted">{fmtSize(batch.size)}</span>
            <Button
              size="sm"
              variant="danger"
              loading={busy === `purge:${batch.name}`}
              disabled={!!busy}
              onClick={() =>
                runBusy(`purge:${batch.name}`, async () => {
                  if (!(await confirmAction({ title: '确认操作', message: `彻底删除批次 ${batch.name}？不可恢复。`, danger: true }))) return;
                  
                    const bytes = await runCmd('maintenance_purge_trash', { tag: batch.name });

                    if (bytes === undefined) return;
                    toast(`已释放 ${fmtSize(bytes)}`);
                    qc.invalidateQueries({ queryKey: ['maint-trash'] });
                    qc.invalidateQueries({ queryKey: ['space'] });
                  
                })
              }
            >
              删除
            </Button>
          </div>
        ))}
        {(trash.data ?? []).length > 0 && (
          <div className="mt-2 flex items-center justify-between">
            <span className="text-xs text-muted">共 {(trash.data ?? []).length} 个批次；删除会话时选「彻底删除」就不会进回收站。</span>
            <Button
              size="sm"
              variant="danger"
              loading={busy === 'purge'}
              disabled={!!busy}
              onClick={() =>
                runBusy('purge', async () => {
                  if (!(await confirmAction({ title: '确认操作', message: '清空整个回收站？不可恢复。', danger: true }))) return;
                  
                    const bytes = await runCmd('maintenance_purge_trash', { tag: null });

                    if (bytes === undefined) return;
                    toast(`已释放 ${fmtSize(bytes)}`);
                    qc.invalidateQueries({ queryKey: ['maint-trash'] });
                    qc.invalidateQueries({ queryKey: ['space'] });
                  
                })
              }
            >
              清空回收站
            </Button>
          </div>
        )}
      </div>

      <div className="rounded-lg border border-line bg-card p-4">
        <h2 className="mb-1 text-sm font-semibold">v1 数据归档收尾</h2>
        <p className="mb-2 text-xs text-muted">
          第一步校验计数是否与库一致（只读）；第二步把 v1 文件**复制**到 data-v1-archive/，原文件永不移动。
        </p>
        {verify && <Notice className="mb-2">{verify}</Notice>}
        <div className="flex gap-2">
          <Button
            size="sm"
            loading={busy === 'verify'}
            disabled={!!busy}
            onClick={() =>
              runBusy('verify', async () => {
                
                  const r = await runCmd('migrate_verify');

                  if (r === undefined) return;
                  const line = (name: string, pair: CountPair) =>
                    `${name} ${pair.v1}/${pair.v2}`;
                  setVerify(
                    `账号 ${line('', r.accounts)} · 会话 ${r.sessions.v1}/${r.sessions.v2} · 积分台账 ${r.ledger.v1}/${r.ledger.v2}\n` +
                      (r.ok ? '计数一致，可以归档。' : '计数不一致，先处理差异再归档。'),
                  );
                
              })
            }
          >
            校验
          </Button>
          <Button
            size="sm"
            variant="primary"
            loading={busy === 'archive'}
            disabled={!!busy}
            onClick={() =>
              runBusy('archive', async () => {
                
                  const r = await runCmd('migrate_archive');

                  if (r === undefined) return;
                  toast(`v1 文件已归档到 ${r.archive_dir}`);
                
              })
            }
          >
            归档
          </Button>
        </div>
      </div>
    </div>
  );
}
