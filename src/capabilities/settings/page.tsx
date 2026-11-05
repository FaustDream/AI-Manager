// 设置中心：通用（主题/开机自启）与维护（定时任务/空间/回收站/打开目录）。
// 日志与执行记录是独立能力（capabilities/logs、capabilities/executions），不在这里。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy, errMsg } from '@/lib/cmd';
import { useEffect, useState } from 'react';

import { fmtSize } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Checkbox, Input, Select, SkeletonRows } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { saveTheme, type ThemeChoice } from '@/lib/theme';
import type { RuleKind, Schedule, ScheduleAction, SpaceEntry, SpaceReport } from '@/lib/types';

// 设置页上下两栏：上面「通用」（主题/开机自启），下面「维护」（定时任务/空间/回收站）。
// 不做左右分栏——左栏只有两个开关却占掉三分之一宽度，右侧表格反而挤。
export default function SettingsPage() {
  return (
    <div className="flex flex-col gap-3">
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
    <div className="rounded-lg border border-line bg-card p-4">
      <h2 className="mb-2 text-sm font-semibold">通用</h2>
      {/* 两个开关横排：拉通整行后单列会留下大片空白 */}
      <div className="grid gap-4 md:grid-cols-2">
        <div>
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
          <p className="mt-1 text-xs text-muted">
            「经典」两套是暖色调：米纸底配棕强调（浅）、暖黑底配琥珀强调（深）；默认两套是冷灰蓝。
            切换即时生效并持久化。
          </p>
        </div>
        <div>
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
          <p className="mt-1 text-xs text-muted">
            写 HKCU\Run，不需要管理员权限；开机后直接进托盘。
          </p>
        </div>
      </div>
      <p className="mt-3 text-xs text-muted">
        日志级别在「日志」页调整；数据目录在下面「数据目录占用」里打开。
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
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <ScheduleCard />

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

    </div>
  );
}

/** 规则人话：与后端 `Schedule::rule_text()` 同一口径。 */
function ruleText(schedule: Schedule): string {
  return schedule.kind === 'daily'
    ? `每天 ${schedule.at ?? '--:--'}`
    : `每 ${schedule.hours ?? 0} 小时`;
}

function shortTs(ts: string | null): string {
  if (!ts) return '从未';
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 定时任务：什么时候自动跑签到/积分，由用户定，不再写死在启动代码里。 */
function ScheduleCard() {
  const qc = useQueryClient();
  const { busy, run: runBusy } = useBusy();
  const list = useQuery<Schedule[]>({
    queryKey: ['schedules'],
    queryFn: () => queryCmd('schedule_list'),
  });
  const [action, setAction] = useState<ScheduleAction>('checkin_round');
  const [kind, setKind] = useState<RuleKind>('daily');
  const [at, setAt] = useState('08:30');
  const [hours, setHours] = useState('6');

  const refresh = () => qc.invalidateQueries({ queryKey: ['schedules'] });

  const add = () =>
    runBusy('schedule-add', async () => {
      const saved = await runCmd('schedule_save', {
        input: {
          id: '',
          name: '',
          action,
          kind,
          at: kind === 'daily' ? at : null,
          hours: kind === 'every' ? Number(hours) || 0 : null,
          enabled: true,
          last_run: null,
        },
      });
      if (saved === undefined) return;
      toast(`已添加：${saved.name} · ${ruleText(saved)}`);
      refresh();
    });

  const toggle = (item: Schedule) =>
    runBusy(`schedule-toggle:${item.id}`, async () => {
      const saved = await runCmd('schedule_save', { input: { ...item, enabled: !item.enabled } });
      if (saved === undefined) return;
      toast(`${saved.name} 已${saved.enabled ? '启用' : '停用'}`);
      refresh();
    });

  const remove = (item: Schedule) =>
    runBusy(`schedule-del:${item.id}`, async () => {
      const ok = await confirmAction({
        title: '删除定时任务',
        message: `删除「${item.name}」？之后不会再自动执行，已有执行记录保留。`,
        danger: true,
      });
      if (!ok) return;
      if ((await runCmd('schedule_delete', { id: item.id })) === undefined) return;
      toast('已删除');
      refresh();
    });

  const runNow = (item: Schedule) =>
    runBusy(`schedule-run:${item.id}`, async () => {
      const message = await runCmd('schedule_run', { id: item.id });
      if (message === undefined) return;
      toast(`${item.name}：${message}`);
      refresh();
      qc.invalidateQueries({ queryKey: ['accounts'] });
      qc.invalidateQueries({ queryKey: ['daily'] });
    });

  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-1 flex items-center gap-2">
        <h2 className="text-sm font-semibold">定时任务</h2>
        <span className="flex-1" />
        <span className="text-xs text-muted">到点自动执行，结果记进「执行记录」</span>
      </div>
      <p className="mb-2 text-xs text-muted">
        关掉就不自动跑，仍可在账号页手动点「批量签到」或这里点「立即执行」。
      </p>

      {list.isLoading && <SkeletonRows rows={2} cols={3} />}
      {(list.data ?? []).length === 0 && <p className="text-sm text-muted">还没有定时任务。</p>}

      {(list.data ?? []).map((item) => (
        <div
          key={item.id}
          className="flex flex-wrap items-center gap-2 border-b border-line px-1 py-1.5 text-sm last:border-b-0"
        >
          <span className="w-24 truncate font-medium" title={item.name}>
            {item.name}
          </span>
          <span className="w-28 text-xs text-muted">{ruleText(item)}</span>
          <span className="w-24 text-xs text-muted">上次 {shortTs(item.last_run)}</span>
          <span className="flex-1" />
          <label className="flex cursor-pointer items-center gap-1 text-xs text-fg2">
            <input
              type="checkbox"
              checked={item.enabled}
              disabled={!!busy}
              onChange={() => toggle(item)}
            />
            启用
          </label>
          <Button
            size="sm"
            loading={busy === `schedule-run:${item.id}`}
            disabled={!!busy}
            onClick={() => runNow(item)}
            title="按这条任务现在跑一次（记一条手动执行记录）"
          >
            立即执行
          </Button>
          <Button
            size="sm"
            variant="danger"
            loading={busy === `schedule-del:${item.id}`}
            disabled={!!busy}
            onClick={() => remove(item)}
          >
            删除
          </Button>
        </div>
      ))}

      <div className="mt-2 flex flex-wrap items-center gap-2">
        <Select value={action} onChange={(e) => setAction(e.target.value as ScheduleAction)}>
          <option value="checkin_round">签到轮询</option>
          <option value="credits_round">积分刷新</option>
        </Select>
        <Select value={kind} onChange={(e) => setKind(e.target.value as RuleKind)}>
          <option value="daily">每天</option>
          <option value="every">每隔</option>
        </Select>
        {kind === 'daily' ? (
          <Input
            className="w-24"
            value={at}
            placeholder="08:30"
            onChange={(e) => setAt(e.target.value)}
            title="触发时刻 HH:mm"
          />
        ) : (
          <Input
            className="w-20"
            value={hours}
            onChange={(e) => setHours(e.target.value)}
            title="间隔小时数"
          />
        )}
        {kind === 'every' && <span className="text-xs text-muted">小时</span>}
        <Button size="sm" variant="primary" loading={busy === 'schedule-add'} disabled={!!busy} onClick={add}>
          添加
        </Button>
      </div>
    </div>
  );
}
