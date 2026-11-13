// 设置中心：通用（主题/开机自启）与维护（定时任务/空间/回收站/打开目录）。
// 日志与执行动作统一在 capabilities/logs，不在这里。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy, errMsg } from '@/lib/cmd';
import { useEffect, useState } from 'react';

import { fmtSize, fmtTime } from '@/lib/format';
import { numOr } from '@/lib/num';
import { Button, cn } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, Input, Select, SkeletonBlock, SkeletonRows } from '@/components/ui/form';
import { DataTable, type Column } from '@/components/ui/table';
import { Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { saveTheme, type ThemeChoice } from '@/lib/theme';
import { HistoryPane } from './history';
import type {
  AppConfig,
  CapabilityMatrix,
  CapabilityRow,
  CloseAction,
  HealthReport,
  RuleKind,
  Schedule,
  ScheduleAction,
  SelfcheckReport,
  SpaceEntry,
  SpaceReport,
  Support,
} from '@/lib/types';

// 设置页分四个标签页，按「要不要动手」排序：需要用户操作的放前面，只需查看的放后面。
//   常规 / 定时任务 / 维护 = 动手改设置、管任务、清回收站；
//   诊断与记录 = 平台能力矩阵、网络自检、历史，以查看为主。
type TabKey = 'general' | 'schedule' | 'maintenance' | 'diagnostics';

export default function SettingsPage() {
  const [tab, setTab] = useState<TabKey>('general');
  return (
    <div className="flex flex-col gap-3">
      <TabBar tab={tab} onChange={setTab} />
      {tab === 'general' && (
        <>
          <GeneralPane />
          <ConfigPane />
        </>
      )}
      {tab === 'schedule' && <ScheduleCard />}
      {tab === 'maintenance' && (
        <>
          <ArchiveEntryPane />
          <RuntimePane />
          <MaintenancePane />
        </>
      )}
      {tab === 'diagnostics' && (
        <>
          <LimitsPane />
          <CapabilityPane />
          <SelfcheckPane />
          <HistoryPane />
        </>
      )}
    </div>
  );
}

function TabBar({ tab, onChange }: { tab: TabKey; onChange: (t: TabKey) => void }) {
  const tabs: { key: TabKey; label: string }[] = [
    { key: 'general', label: '常规' },
    { key: 'schedule', label: '定时任务' },
    { key: 'maintenance', label: '维护' },
    { key: 'diagnostics', label: '诊断与记录' },
  ];
  return (
    <div className="flex flex-wrap gap-1 rounded-lg border border-line bg-card p-1">
      {tabs.map((t) => (
        <button
          key={t.key}
          type="button"
          onClick={() => onChange(t.key)}
          className={cn(
            'rounded-md px-4 py-1.5 text-sm transition-colors',
            tab === t.key ? 'bg-accent text-white' : 'text-fg2 hover:bg-hover',
          )}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

function GeneralPane() {
  const [theme, setTheme] = useState<ThemeChoice>('');
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [version, setVersion] = useState('');

  useEffect(() => {
    queryCmd('app_version').then(setVersion).catch(() => {});
    queryCmd('settings_get', { key: 'ui.theme' })
      .then((v) => setTheme((v as ThemeChoice) || ''))
      .catch(() => {});
    queryCmd('autostart_get')
      .then(setAutostart)
      .catch(() => setAutostart(false));
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
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">关闭按钮</span>
            <Select
              value={draft.window.close_action}
              onChange={(e) => patch({ window: { ...draft.window, close_action: e.target.value as CloseAction } })}
            >
              <option value="tray">最小化到托盘（继续跑定时任务）</option>
              <option value="exit">直接退出程序</option>
            </Select>
          </label>
          <p className="mt-1 pl-32 text-xs text-muted">
            {draft.window.close_action === 'tray'
              ? '点 X 后窗口收进托盘，进程常驻；要退出请右键托盘图标 →「退出」。'
              : '点 X 直接结束进程，定时签到不会继续跑；需要常驻请改回托盘。'}
          </p>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <span className="w-28 shrink-0 text-muted">自启最小化</span>
            <Checkbox
              checked={draft.window.start_minimized}
              onChange={(e) => patch({ window: { ...draft.window, start_minimized: e.target.checked } })}
            />
            <span className="text-xs text-muted">开机自启启动时收进托盘，不弹主窗口</span>
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
 * 「已知限制」：平台侧的硬约束，本应用改不了的，集中说清楚。
 * 口径与 `app/capability.rs` 的矩阵一致，但这里给的是人话 + 替代做法：
 * 用户在「添加账号」里找不到 Trae 时，第一反应是「是不是坏了」，这里要能直接回答。
 */
const KNOWN_LIMITS: { scope: string; title: string; detail: string; workaround?: string }[] = [
  {
    scope: 'Trae',
    title: '登录态只读：不能添加 / 切换账号',
    detail:
      '登录态与设备绑定、加密只读。写回去会把 Trae 客户端登出，所以本应用不提供「添加账号」「切换登录」。',
    workaround:
      '在 Trae 客户端换号登录一次，再启动本应用（或点账号中心刷新），扫描会自动入库。旧账号记录会保留，但无法一键切回。',
  },
  {
    scope: 'Trae',
    title: '新版会话库加密',
    detail: '新版会话库是 SQLCipher 加密，本应用只能读旧版 state.vscdb，或从插件运行日志解析。',
    workaround: '需要完整会话时，在 Trae 里导出，或保留旧版数据。',
  },
  {
    scope: 'WorkBuddy 国际版',
    title: '没有签到玩法，也不显示批次到期',
    detail: '国际版只有积分查询；批次到期接口不提供可靠字段，所以国际版不显示到期日。',
  },
  {
    scope: 'ZCode',
    title: '额度型平台，不是积分余额',
    detail: '显示各额度窗口的已用百分比；活动奖励要在账号详情页手动领取。',
  },
  {
    scope: 'CodeBuddy',
    title: '账号体系跟随 WorkBuddy',
    detail: '不单独登录；用 WorkBuddy 国内版档案切换 CodeBuddy IDE 与插件，不支持国际版账号。',
  },
  {
    scope: '通用',
    title: '客户端里也过期的凭据',
    detail: '「刷新凭据」只重扫客户端本地登录态；客户端自己也过期了，只能去重新登录一次。',
  },
];

function LimitsPane() {
  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex items-center gap-2">
        <h2 className="text-sm font-semibold">已知限制</h2>
        <span className="flex-1" />
        <span className="text-xs text-muted">平台侧约束，本应用无法绕过</span>
      </div>
      <p className="mb-3 text-xs text-muted">
        下面这些是平台本身的硬限制（登录态设备绑定、接口不提供、加密格式等），不是本应用没做。
        遇到对应情况时，按「怎么做」处理即可。
      </p>
      <div className="flex flex-col gap-2">
        {KNOWN_LIMITS.map((item) => (
          <div key={`${item.scope}-${item.title}`} className="rounded-md border border-line px-3 py-2">
            <div className="flex flex-wrap items-baseline gap-2">
              <span className="rounded bg-hover px-1.5 text-xs text-fg2">{item.scope}</span>
              <span className="text-sm font-medium">{item.title}</span>
            </div>
            <p className="mt-1 text-xs text-muted">{item.detail}</p>
            {item.workaround && (
              <p className="mt-1 text-xs">
                <span className="text-muted">怎么做：</span>
                {item.workaround}
              </p>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

/**
 * 「平台能力」：一张矩阵说清哪家能做什么、哪家做不了、为什么。
 *
 * 数据来自后端 `app/capability.rs` 的唯一真源表——前端不判断平台，
 * 只把 state 渲染成符号。这样「平台限制」和「本应用还没做」不会再被混为一谈。
 */
const SUPPORT_MARK: Record<Support, { mark: string; cls: string; label: string }> = {
  full: { mark: '✓', cls: 'text-ok', label: '支持' },
  partial: { mark: '△', cls: 'text-warn', label: '部分支持' },
  none: { mark: '—', cls: 'text-muted', label: '不支持' },
};

function CapabilityPane() {
  const matrix = useQuery<CapabilityMatrix>({
    queryKey: ['capabilities'],
    queryFn: () => queryCmd('platform_capabilities'),
    // 静态表：一次进程生命周期内不会变，没必要反复拉
    staleTime: Infinity,
  });
  const data = matrix.data;

  const columns: Column<CapabilityRow>[] = data
    ? [
        {
          key: 'platform',
          header: '平台',
          width: 150,
          cell: (row) => <span className="whitespace-nowrap font-medium">{row.label}</span>,
        },
        ...data.features.map((feature, index) => ({
          key: feature.key,
          header: <span title={feature.hint}>{feature.label}</span>,
          width: 82,
          cell: (row: CapabilityRow) => {
            const cell = row.cells[index];
            const meta = SUPPORT_MARK[cell.state];
            return (
              <span
                className={`cursor-help ${meta.cls}`}
                title={cell.note || `${meta.label}：${feature.hint}`}
              >
                {meta.mark}
              </span>
            );
          },
        })),
      ]
    : [];

  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <h2 className="text-sm font-semibold">平台能力</h2>
        <span className="flex-1" />
        <span className="flex items-center gap-3 text-xs text-muted">
          <span className="text-ok">✓ 支持</span>
          <span className="text-warn">△ 部分支持</span>
          <span>— 不支持</span>
        </span>
      </div>
      <p className="mb-2 text-xs text-muted">
        悬停任意格子看具体限制。「不支持」是平台本身的约束（登录态设备绑定、没有签到玩法等），
        不是本应用没做——需要去客户端操作的那几项，格子里会写清走哪条路。
      </p>
      {!data && (
        <table className="w-full border-collapse">
          <tbody>
            <SkeletonRows rows={5} cols={6} />
          </tbody>
        </table>
      )}
      {data && (
        <div className="overflow-x-auto">
          <DataTable
            dense
            columns={columns}
            rows={data.platforms}
            rowKey={(row) => row.key}
            empty={null}
          />
        </div>
      )}
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
        {space.isLoading && <SkeletonBlock rows={3} />}
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

/** 已归档会话的入口：归档列表已拆成独立页（manifest.hidden，不占顶部导航），从设置进入。 */
function ArchiveEntryPane() {
  // 带上条数：自动归档是按时间静默跑的，不在这儿显示条数，用户根本不知道它有没有生效
  const archived = useQuery({
    queryKey: ['sessions', 'archive', 'entry-total'],
    queryFn: () =>
      queryCmd('session_page', {
        cursor: null,
        limit: 1,
        filter: { source: null, account: null, state: 'archived', q: null, origin: null, sort: 'last_at' },
      }),
  });
  const total = archived.data?.total ?? 0;
  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="text-sm font-semibold">已归档会话</h2>
        <span className="text-xs text-muted">
          {archived.isLoading ? '统计中…' : total > 0 ? `已归档 ${total} 条` : '还没有归档的会话'}
        </span>
        <span className="flex-1" />
        <Button
          size="sm"
          onClick={() =>
            window.dispatchEvent(new CustomEvent('aim:navigate', { detail: 'sessions-archive' }))
          }
        >
          打开已归档会话
        </Button>
      </div>
      <p className="mt-1 text-xs text-muted">
        归档只写本地标记、不删文件；在独立页里可以预览、取消归档或彻底删除，支持一键全选与 Ctrl 多选。
        自动归档按「最后活动时间」算（不是创建时间）：超过设定天数没动过、且还出现在会话列表里的，会被收进来。
      </p>
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
  if (schedule.kind === 'daily') return `每天 ${schedule.at ?? '--:--'}`;
  // minutes 优先，没有才回落到 hours：老配置只有 hours
  const minutes = schedule.minutes ?? (schedule.hours ?? 0) * 60;
  if (minutes <= 0) return '每隔 --';
  return minutes >= 60 && minutes % 60 === 0
    ? `每 ${minutes / 60} 小时`
    : `每 ${minutes} 分钟`;
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
  // 间隔拆成「数值 + 单位」：积分要跟得紧，小时粒度不够用
  const [every, setEvery] = useState('5');
  const [unit, setUnit] = useState<'minute' | 'hour'>('minute');
  const [editing, setEditing] = useState<Schedule | null>(null);

  const edit = (item: Schedule) => {
    setEditing(item);
    setAction(item.action);
    setKind(item.kind);
    setName(item.name);
    setAt(item.at ?? '08:30');
    const minutes = item.minutes ?? (item.hours ?? 0) * 60;
    if (minutes > 0 && minutes % 60 === 0) {
      setUnit('hour');
      setEvery(String(minutes / 60));
    } else {
      setUnit('minute');
      setEvery(String(minutes || 5));
    }
  };

  const resetForm = () => {
    setEditing(null);
    setName('');
    setAt('08:30');
    setEvery('5');
    setUnit('minute');
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
          // 只传当前单位那一项：后端按「分钟优先」折算，另一项由它清空
          minutes: kind === 'every' && unit === 'minute' ? Number(every) || 0 : null,
          hours: kind === 'every' && unit === 'hour' ? Number(every) || 0 : null,
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

      {list.isLoading && <SkeletonBlock rows={3} />}
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
          <>
            <Input
              className="w-20"
              type="number"
              aria-label="间隔数值"
              min={1}
              max={unit === 'hour' ? 168 : 10080}
              value={every}
              onChange={(e) => setEvery(e.target.value)}
              title={unit === 'hour' ? '间隔小时数（1–168）' : '间隔分钟数（1–10080）'}
            />
            <Select
              aria-label="间隔单位"
              value={unit}
              disabled={!!busy}
              onChange={(e) => setUnit(e.target.value as 'minute' | 'hour')}
            >
              <option value="minute">分钟</option>
              <option value="hour">小时</option>
            </Select>
          </>
        )}
        <Button size="sm" variant="primary" loading={busy === 'schedule-add'} disabled={!!busy} onClick={add}>
          {editing ? '保存' : '添加'}
        </Button>
        {editing && <Button size="sm" variant="ghost" disabled={!!busy} onClick={resetForm}>取消</Button>}
      </div>
    </div>
  );
}
