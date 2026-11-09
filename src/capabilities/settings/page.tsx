// 设置中心：通用（主题/开机自启）与维护（定时任务/空间/回收站/打开目录）。
// 日志与执行动作统一在 capabilities/logs，不在这里。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy, errMsg } from '@/lib/cmd';
import { useEffect, useState } from 'react';

import { fmtSize, fmtTime } from '@/lib/format';
import { numOr } from '@/lib/num';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, Input, Select, SkeletonRows } from '@/components/ui/form';
import { Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { saveTheme, type ThemeChoice } from '@/lib/theme';
import { HistoryPane } from './history';
import type { AppConfig, HealthReport, RuleKind, Schedule, ScheduleAction, SelfcheckReport, SpaceEntry, SpaceReport } from '@/lib/types';

// 设置页按「改什么」自上而下：通用 → 配置 → 自检 → 运行与存储 → 维护 → 可追溯。
// 不做左右分栏——左栏只有两个开关却占掉三分之一宽度，右侧表格反而挤。
export default function SettingsPage() {
  return (
    <div className="flex flex-col gap-3">
      <GeneralPane />
      <ConfigPane />
      <SelfcheckPane />
      <RuntimePane />
      <MaintenancePane />
      <HistoryPane />
    </div>
  );
}

function GeneralPane() {
  const [theme, setTheme] = useState<ThemeChoice>('');
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [version, setVersion] = useState('');
  // 技能页是否显示官方自带条目（kv ui.hub.show-official，缺省显示）
  const [official, setOfficial] = useState(true);

  useEffect(() => {
    queryCmd('app_version').then(setVersion).catch(() => {});
    queryCmd('settings_get', { key: 'ui.theme' })
      .then((v) => setTheme((v as ThemeChoice) || ''))
      .catch(() => {});
    queryCmd('autostart_get')
      .then(setAutostart)
      .catch(() => setAutostart(false));
    queryCmd('settings_get', { key: 'ui.hub.show-official' })
      .then((v) => setOfficial(v !== false))
      .catch(() => {});
  }, []);

  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex items-center gap-2">
        <h2 className="text-sm font-semibold">通用</h2>
        <span className="flex-1" />
        {version && <span className="text-xs text-muted">当前版本 v{version}</span>}
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
            <span className="w-24 text-muted">官方条目</span>
            <Checkbox
              checked={official}
              onChange={async (e) => {
                const on = e.target.checked;
                setOfficial(on);
                try {
                  await queryCmd('settings_set', { key: 'ui.hub.show-official', value: on });
                  toast(on ? '技能页将显示官方自带条目' : '技能页已隐藏官方自带条目');
                } catch (err) {
                  setOfficial(!on);
                  toast(errMsg(err));
                }
              }}
            />
            <span className="text-xs text-muted">技能页显示各软件自带的技能与 MCP</span>
          </label>
        </div>
      </div>
      <p className="mt-2 text-xs text-muted">
        签到节奏、网络与采集间隔等运行配置已移到下方「配置」区统一管理。
      </p>
    </div>
  );
}

/**
 * 「配置」区：应用运行参数的唯一调整入口（data/config.toml 的界面通道）。
 * 保存 = config_set：立即生效并写回文件；手改文件则重启生效。
 * 内容分级：数字输入 = 精调项，开关 = 显式授权项，保存按钮 = 唯一主操作。
 */
function ConfigPane() {
  const qc = useQueryClient();
  const { busy, run: runBusy } = useBusy();
  const [draft, setDraft] = useState<AppConfig | null>(null);

  useEffect(() => {
    queryCmd('config_get')
      .then(setDraft)
      .catch(() => {});
  }, []);

  if (!draft) return null;

  const patch = (change: Partial<AppConfig>) => setDraft({ ...draft, ...change });

  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex items-center gap-2">
        <h2 className="text-sm font-semibold">配置</h2>
        <span className="flex-1" />
        <span className="text-xs text-muted">配置文件：data\config.toml（手改后重启生效）</span>
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">签到并发</span>
            <Select
              value={String(draft.checkin.max_concurrent)}
              onChange={(e) => patch({ checkin: { ...draft.checkin, max_concurrent: numOr(e.target.value, 3) } })}
            >
              {[1, 2, 3, 4, 5, 6].map((n) => (
                <option key={n} value={n}>
                  {n} 个
                </option>
              ))}
            </Select>
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">账号间隔</span>
            <Select
              value={String(draft.checkin.gap_ms)}
              onChange={(e) => patch({ checkin: { ...draft.checkin, gap_ms: numOr(e.target.value, 0) } })}
            >
              {[0, 500, 1000, 2000, 5000].map((ms) => (
                <option key={ms} value={ms}>
                  {ms === 0 ? '不间隔' : `${ms / 1000} 秒`}
                </option>
              ))}
            </Select>
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">请求超时</span>
            <Input
              type="number"
              min={3}
              max={120}
              className="w-24"
              value={draft.network.timeout_secs}
              onChange={(e) => patch({ network: { ...draft.network, timeout_secs: numOr(e.target.value, 15) } })}
            />
            <span className="text-xs text-muted">秒（走代理调慢时可调大）</span>
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">GET 重试</span>
            <Select
              value={String(draft.network.get_retry)}
              onChange={(e) => patch({ network: { ...draft.network, get_retry: numOr(e.target.value, 1) } })}
            >
              {[0, 1, 2, 3].map((n) => (
                <option key={n} value={n}>
                  {n} 次
                </option>
              ))}
            </Select>
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">用量采集间隔</span>
            <Select
              value={String(draft.usage.scan_interval_min)}
              onChange={(e) => patch({ usage: { ...draft.usage, scan_interval_min: numOr(e.target.value, 30) } })}
            >
              {[10, 15, 30, 60, 120].map((n) => (
                <option key={n} value={n}>
                  {n} 分钟
                </option>
              ))}
            </Select>
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">仪表盘补刷</span>
            <Select
              value={String(draft.dashboard.stale_refresh_min)}
              onChange={(e) => patch({ dashboard: { ...draft.dashboard, stale_refresh_min: numOr(e.target.value, 10) } })}
            >
              {[5, 10, 15, 30, 60].map((n) => (
                <option key={n} value={n}>
                  数据超过 {n} 分钟
                </option>
              ))}
            </Select>
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">旅行记录条数</span>
            <Input
              type="number"
              min={10}
              max={500}
              className="w-24"
              value={draft.activity.travel_events_limit}
              onChange={(e) => patch({ activity: { ...draft.activity, travel_events_limit: numOr(e.target.value, 50) } })}
            />
            <span className="text-xs text-muted">每账号保留（10-500）</span>
          </label>
        </div>
        <div className="flex items-center">
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">ZCode 自动领取</span>
            <Checkbox
              checked={draft.activity.zcode_auto_claim}
              onChange={(e) => patch({ activity: { ...draft.activity, zcode_auto_claim: e.target.checked } })}
            />
            <span className="text-xs text-muted">每 15 分钟探测一轮，领取要过浏览器验证码</span>
          </label>
        </div>
      </div>
      <div className="mt-3 flex items-center gap-2">
        <Button
          size="sm"
          variant="primary"
          loading={busy === 'config'}
          disabled={busy === 'config'}
          onClick={() =>
            runBusy('config', async () => {
              if ((await runCmd('config_set', { config: draft })) === undefined) return;
              toast('配置已保存并生效（同时写入 data/config.toml）');
              await qc.invalidateQueries({ queryKey: ['config'] });
            })
          }
        >
          保存配置
        </Button>
        <Button size="sm" variant="ghost" disabled={busy === 'config'} onClick={() => void queryCmd('config_get').then(setDraft)}>
          还原
        </Button>
        <span className="flex-1" />
        <span className="text-xs text-muted">并发实际执行收敛到 1-6，间隔上限 60 秒，越界值保存时按此修正</span>
      </div>
    </div>
  );
}


/**
 * 「网络自检」：国际版链路排障的第一入口（口径与 tools/intl-doctor.ps1 一致）。
 * 三段信息按「要不要动手」排：代理开关 → 端点可达性 → 档案完整性。
 * 端点 401/403 算「通」（链路好、只是没带凭据），5xx/超时才算不通。
 */
function SelfcheckPane() {
  const { busy, run: runBusy } = useBusy();
  const [report, setReport] = useState<SelfcheckReport | null>(null);

  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex items-center gap-2">
        <h2 className="text-sm font-semibold">网络自检</h2>
        <span className="flex-1" />
        <Button size="sm" loading={busy === 'selfcheck'} disabled={busy === 'selfcheck'} onClick={() =>
          runBusy('selfcheck', async () => {
            const report = await runCmd('network_selfcheck');
            if (report !== undefined) setReport(report);
          })
        }>
          {busy === 'selfcheck' ? '检测中…' : '运行自检'}
        </Button>
      </div>
      {!report && (
        <p className="text-xs text-muted">
          国际版账号报「网络错误」时先跑这里：一次看清新系代理是否开启、三个国际端点是否可达、登录态档案是否完整。
        </p>
      )}
      {report && (
        <div className="flex flex-col gap-1.5 text-xs">
          <div className="flex flex-wrap items-center gap-2">
            <Tag tone={report.proxy_enabled ? 'ok' : 'warn'}>
              {report.proxy_enabled ? '系统代理已开启' : '系统代理未开启'}
            </Tag>
            {report.proxy_server && (
              <span className="font-mono text-muted" title="WinINET ProxyServer">
                {report.proxy_server}
              </span>
            )}
            {!report.proxy_enabled && (
              <span className="text-muted">国际版直连必然超时——先开代理再测</span>
            )}
          </div>
          {report.endpoints.map((endpoint) => (
            <div key={endpoint.url} className="flex flex-wrap items-center gap-2">
              <Tag tone={endpoint.ok ? 'ok' : 'bad'}>{endpoint.ok ? '可达' : '不通'}</Tag>
              <span className="font-mono" title={endpoint.url}>
                {endpoint.url.replace('https://', '').replace(/\/$/, '')}
              </span>
              <span className="text-muted">{endpoint.detail}</span>
            </div>
          ))}
          {report.profiles.length > 0 && (
            <>
              <div className="mt-1 font-medium text-fg2">WorkBuddy 登录态档案</div>
              {report.profiles.map((profile) => (
                <div key={profile.label} className="flex flex-wrap items-center gap-2">
                  <Tag tone={profile.complete ? 'ok' : profile.tmp_only ? 'muted' : 'bad'}>
                    {profile.complete ? '完整' : profile.tmp_only ? '待认领' : '缺件'}
                  </Tag>
                  <span className="font-mono text-muted">{profile.label}</span>
                  {profile.tmp_only && (
                    <span className="text-muted">下次切换自动认领，不用管</span>
                  )}
                  {!profile.complete && !profile.tmp_only && (
                    <span className="text-muted">在客户端登录一次该账号即可修复</span>
                  )}
                </div>
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * 「运行与存储」：从仪表盘迁来——仪表盘只留「要不要动手」的结论，运行细节归设置页。
 * 数据目录逐项占用与健康报告合并成一张卡：原来分居两页，现在一个入口看全。
 */
function RuntimePane() {
  const health = useQuery<HealthReport>({
    queryKey: ['health'],
    queryFn: () => queryCmd('system_health'),
    retry: 1,
    retryDelay: 6_000,
    refetchInterval: (query) => (query.state.error ? 300_000 : 60_000),
  });
  const space = useQuery<SpaceReport>({
    queryKey: ['space'],
    queryFn: () => queryCmd('maintenance_space'),
  });
  const report = health.data;

  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <h2 className="text-sm font-semibold">运行与存储</h2>
        <span className="flex-1" />
        {space.data && <span className="text-xs text-muted">合计 {fmtSize(space.data.total)}</span>}
        <Button size="sm" onClick={() => void runCmd('maintenance_open_data')}>
          打开数据目录
        </Button>
      </div>

      {report && (
        <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
          <dt className="text-muted">数据目录</dt>
          <dd className="truncate font-mono" title={report.data_dir}>
            {report.data_dir}
          </dd>
          <dt className="text-muted">平台</dt>
          <dd className="flex flex-wrap gap-1">
            {report.platforms.map((platform) => (
              <span
                key={platform.platform}
                className={
                  platform.failing > 0
                    ? 'rounded bg-bad/15 px-1 text-bad'
                    : 'rounded bg-hover px-1 text-fg2'
                }
                title={`最近成功：${fmtTime(platform.last_ok_at)} · 最近失败：${fmtTime(platform.last_fail_at)}`}
              >
                {platform.label} {platform.accounts}
                {platform.failing > 0 ? ` · 失败 ${platform.failing}` : ''}
              </span>
            ))}
          </dd>
        </dl>
      )}

      <div className="mt-2 border-t border-line pt-1">
        {space.isLoading && <SkeletonRows rows={4} cols={2} />}
        {(space.data?.entries ?? []).map((entry) => (
          <div
            key={entry.name}
            className="flex items-center gap-2 border-b border-line/60 px-1 py-1 text-sm last:border-b-0"
          >
            <span className="flex-1 truncate" title={entry.path}>
              {entry.name}
            </span>
            <span className="tabular-nums text-muted">{fmtSize(entry.size)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function MaintenancePane() {
  const qc = useQueryClient();
  const { busy, run: runBusy } = useBusy();
  const trash = useQuery<SpaceEntry[]>({
    queryKey: ['maint-trash'],
    queryFn: () => queryCmd('maintenance_trash'),
  });
  // 批次明细收进对话框：设置页只留一行摘要
  const [trashOpen, setTrashOpen] = useState(false);
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <ScheduleCard />

      {/* 摘要行：批次明细与逐批删除收进对话框，设置页不再被回收站列表撑高 */}
      <div className="rounded-lg border border-line bg-card p-4">
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="text-sm font-semibold">平台技能回收站</h2>
          <span className="text-xs text-muted">
            {(trash.data ?? []).length > 0
              ? `${(trash.data ?? []).length} 个批次 · 共 ${fmtSize((trash.data ?? []).reduce((sum, batch) => sum + batch.size, 0))}`
              : '空的。'}
          </span>
          <span className="flex-1" />
          {(trash.data ?? []).length > 0 && (
            <>
              <Button size="sm" onClick={() => setTrashOpen(true)}>
                查看批次
              </Button>
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
                清空
              </Button>
            </>
          )}
        </div>
      </div>

      <Dialog
        open={trashOpen}
        title="平台技能回收站"
        subtitle="移除平台技能时副本先放这里；会话删除不进回收站"
        onClose={() => setTrashOpen(false)}
        wide
        footer={
          <>
            {(trash.data ?? []).length > 0 && (
              <Button
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
            )}
            <Button onClick={() => setTrashOpen(false)}>关闭</Button>
          </>
        }
      >
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
      </Dialog>

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
  const [name, setName] = useState('');
  const [at, setAt] = useState('08:30');
  const [hours, setHours] = useState('6');
  const [editing, setEditing] = useState<Schedule | null>(null);

  const edit = (item: Schedule) => {
    setEditing(item);
    setAction(item.action);
    setKind(item.kind);
    setName(item.name);
    setAt(item.at ?? '08:30');
    setHours(String(item.hours ?? 6));
  };

  const resetForm = () => {
    setEditing(null);
    setName('');
    setAt('08:30');
    setHours('6');
  };

  const refresh = () => qc.invalidateQueries({ queryKey: ['schedules'] });

  const add = () =>
    runBusy('schedule-add', async () => {
      const saved = await runCmd('schedule_save', {
        input: {
          ...(editing ?? { id: '', name: '', enabled: true, last_run: null, account_iid: null, created_at: null }),
          // 名称留空交给后端按动作补全；时间值来自原生控件，格式天然合法
          name,
          action,
          kind,
          at: kind === 'daily' ? at : null,
          hours: kind === 'every' ? Number(hours) || 0 : null,
        },
      });
      if (saved === undefined) return;
      toast(`已${editing ? '保存' : '添加'}：${saved.name} · ${ruleText(saved)}`);
      resetForm();
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
        <Input
          className="w-40"
          aria-label="任务名称"
          value={name}
          placeholder="任务名称（可留空自动命名）"
          onChange={(e) => setName(e.target.value)}
        />
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
          // 原生时间控件：产出恒为 HH:mm，后端不再需要挡格式错误
          <Input
            className="w-28"
            type="time"
            aria-label="触发时刻"
            value={at}
            onChange={(e) => setAt(e.target.value)}
            title="每天触发的时刻"
          />
        ) : (
          <Input
            className="w-20"
            type="number"
            aria-label="间隔小时"
            min={1}
            max={168}
            value={hours}
            onChange={(e) => setHours(e.target.value)}
            title="间隔小时数（1–168）"
          />
        )}
        {kind === 'every' && <span className="text-xs text-muted">小时</span>}
        <Button size="sm" variant="primary" loading={busy === 'schedule-add'} disabled={!!busy} onClick={add}>
          {editing ? '保存' : '添加'}
        </Button>
        {editing && <Button size="sm" variant="ghost" disabled={!!busy} onClick={resetForm}>取消</Button>}
      </div>
    </div>
  );
}
