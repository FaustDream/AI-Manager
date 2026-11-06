// 设置中心：通用（主题/开机自启）与维护（定时任务/空间/回收站/打开目录）。
// 日志与执行动作统一在 capabilities/logs，不在这里。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy, errMsg } from '@/lib/cmd';
import { useEffect, useState } from 'react';

import { fmtSize } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Checkbox, Input, Select, SkeletonRows } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { saveTheme, type ThemeChoice } from '@/lib/theme';
import { HistoryPane } from './history';
import type { RuleKind, Schedule, ScheduleAction, SpaceEntry, SpaceReport } from '@/lib/types';

// 设置页上下两栏：上面「通用」（主题/开机自启），下面「维护」（定时任务/空间/回收站）。
// 不做左右分栏——左栏只有两个开关却占掉三分之一宽度，右侧表格反而挤。
export default function SettingsPage() {
  return (
    <div className="flex flex-col gap-3">
      <GeneralPane />
      <MaintenancePane />
      <HistoryPane />
    </div>
  );
}

function GeneralPane() {
  const [theme, setTheme] = useState<ThemeChoice>('');
  const [autostart, setAutostart] = useState<boolean | null>(null);
  // 签到节奏：并发上限 + 账号间隔。默认 3/0，与旧行为一致。
  const [concurrency, setConcurrency] = useState(3);
  const [gapMs, setGapMs] = useState(0);
  // ZCode 活动自动领取：kv 开关，默认关。领取要开浏览器过验证码，必须由用户主动打开。
  const [zcodeAutoClaim, setZcodeAutoClaim] = useState(false);
  const [version, setVersion] = useState('');

  useEffect(() => {
    queryCmd('app_version').then(setVersion).catch(() => {});
    queryCmd('settings_get', { key: 'ui.theme' })
      .then((v) => setTheme((v as ThemeChoice) || ''))
      .catch(() => {});
    queryCmd('autostart_get')
      .then(setAutostart)
      .catch(() => setAutostart(false));
    queryCmd('settings_get', { key: 'checkin.max_concurrent' })
      .then((v) => setConcurrency(Number(v) || 3))
      .catch(() => {});
    queryCmd('settings_get', { key: 'checkin.gap_ms' })
      .then((v) => setGapMs(Number(v) || 0))
      .catch(() => {});
    queryCmd('settings_get', { key: 'zcode_auto_claim' })
      .then((v) => setZcodeAutoClaim(v === true))
      .catch(() => {});
  }, []);

  const savePacing = (key: string, value: number, apply: (n: number) => void) => {
    apply(value);
    void queryCmd('settings_set', { key, value })
      .then(() => toast('签到节奏已保存，下一轮批量生效'))
      .catch((e) => toast(errMsg(e)));
  };

  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex items-center gap-2">
        <h2 className="text-sm font-semibold">通用</h2>
        <span className="flex-1" />
        {version && <span className="text-xs text-muted">AI Manager v{version}</span>}
      </div>
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
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-24 text-muted">签到并发</span>
            <Select
              value={String(concurrency)}
              onChange={(e) => savePacing('checkin.max_concurrent', Number(e.target.value), setConcurrency)}
            >
              {[1, 2, 3, 4].map((n) => (
                <option key={n} value={n}>
                  {n} 个
                </option>
              ))}
            </Select>
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-24 text-muted">账号间隔</span>
            <Select
              value={String(gapMs)}
              onChange={(e) => savePacing('checkin.gap_ms', Number(e.target.value), setGapMs)}
            >
              {[0, 500, 1000, 2000, 5000].map((ms) => (
                <option key={ms} value={ms}>
                  {ms === 0 ? '不间隔' : `${ms / 1000} 秒`}
                </option>
              ))}
            </Select>
          </label>
        </div>
        <div className="md:col-span-2">
          <label className="flex items-center gap-4 text-sm">
            <span className="w-24 text-muted">ZCode 自动领取</span>
            <Checkbox
              checked={zcodeAutoClaim}
              onChange={(e) => {
                const enabled = e.target.checked;
                setZcodeAutoClaim(enabled);
                void queryCmd('settings_set', { key: 'zcode_auto_claim', value: enabled })
                  .then(() =>
                    toast(
                      enabled
                        ? '已开启：每 15 分钟探测一次，有可领活动时会打开浏览器过验证码'
                        : '已关闭 ZCode 自动领取',
                    ),
                  )
                  .catch((err) => {
                    setZcodeAutoClaim(!enabled);
                    toast(errMsg(err));
                  });
              }}
            />
            <span className="text-xs text-muted">
              每 15 分钟探测一轮；领取要你在浏览器里过验证码，所以默认关闭
            </span>
          </label>
        </div>
      </div>
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
          <h2 className="text-sm font-semibold">平台技能回收站</h2>
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
            <span className="text-xs text-muted">共 {(trash.data ?? []).length} 个批次；移除平台技能时副本会先放这里。会话删除不进回收站。</span>
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
  const [editing, setEditing] = useState<Schedule | null>(null);

  const edit = (item: Schedule) => {
    setEditing(item);
    setAction(item.action);
    setKind(item.kind);
    setAt(item.at ?? '08:30');
    setHours(String(item.hours ?? 6));
  };

  const refresh = () => qc.invalidateQueries({ queryKey: ['schedules'] });

  const add = () =>
    runBusy('schedule-add', async () => {
      const saved = await runCmd('schedule_save', {
        input: {
          ...(editing ?? { id: '', name: '', enabled: true, last_run: null, account_iid: null, created_at: null }),
          action,
          kind,
          at: kind === 'daily' ? at : null,
          hours: kind === 'every' ? Number(hours) || 0 : null,
        },
      });
      if (saved === undefined) return;
      toast(`已${editing ? '保存' : '添加'}：${saved.name} · ${ruleText(saved)}`);
      setEditing(null);
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
        message: `删除「${item.name}」？之后不会再自动执行，已有执行动作保留。`,
        danger: true,
      });
      if (!ok) return;
      if ((await runCmd('schedule_delete', { id: item.id })) === undefined) return;
      toast('已删除');
      if (editing?.id === item.id) setEditing(null);
      refresh();
      qc.invalidateQueries({ queryKey: ['activity'] });
    });

  const runNow = (item: Schedule) =>
    runBusy(`schedule-run:${item.id}`, async () => {
      const message = await runCmd('schedule_run', { id: item.id });
      if (message === undefined) return;
      toast(`${item.name}：${message}`);
      refresh();
      qc.invalidateQueries({ queryKey: ['accounts'] });
      qc.invalidateQueries({ queryKey: ['daily'] });
      qc.invalidateQueries({ queryKey: ['detail'] });
      qc.invalidateQueries({ queryKey: ['activity'] });
    });

  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex items-center gap-2">
        <h2 className="text-sm font-semibold">定时任务</h2>
      </div>

      {list.isLoading && <SkeletonRows rows={2} cols={3} />}
      {(list.data ?? []).length === 0 && <p className="text-sm text-muted">还没有定时任务。</p>}

      {(list.data ?? []).map((item) => (
        <div
          key={item.id}
          className="flex flex-wrap items-center gap-2 border-b border-line px-1 py-1.5 text-sm last:border-b-0"
        >
          <span className="w-48 max-w-full truncate font-medium" title={item.name}>
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
          <Button size="sm" disabled={!!busy} onClick={() => edit(item)}>
            编辑
          </Button>
          <Button
            size="sm"
            loading={busy === `schedule-run:${item.id}`}
            disabled={!!busy}
            onClick={() => runNow(item)}
            title="按这条任务现在跑一次（记一条手动执行动作）"
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
        {editing && <span className="text-xs font-semibold">编辑：{editing.name}</span>}
        <Select aria-label="任务动作" value={action} disabled={!!editing} onChange={(e) => setAction(e.target.value as ScheduleAction)}>
          <option value="checkin_round">签到轮询</option>
          <option value="credits_round">积分刷新</option>
          <option value="travel_round">猫猫旅行</option>
        </Select>
        <Select aria-label="执行频率" value={kind} disabled={!!busy} onChange={(e) => setKind(e.target.value as RuleKind)}>
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
          {editing ? '保存' : '添加'}
        </Button>
        {editing && <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => setEditing(null)}>取消</Button>}
      </div>
    </div>
  );
}
