// 设置中心：通用（主题/桌面通知）与维护（定时任务/空间/回收站/打开目录）。
// 日志与执行动作统一在 capabilities/logs，不在这里。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useMemo, useState } from 'react';
import { CalendarClock, Info, SlidersHorizontal, Wrench, type LucideIcon } from 'lucide-react';
import { capabilities } from '@/app/router';
import { usePageToggles } from '@/lib/ui-state';

import { fmtSize, fmtTime } from '@/lib/format';
import { numOr } from '@/lib/num';
import { Button } from '@/components/ui/button';
import { Section } from '@/components/ui/card';
import { Dialog } from '@/components/ui/dialog';
import { Input, SkeletonBlock, SkeletonRows, StateBlock } from '@/components/ui/form';
import { DataTable, type Column } from '@/components/ui/table';
import { Hint, Picker, Switch, Tabs, Tag, Tooltip } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import {
  disableNotify,
  enableNotify,
  notifyEnabled,
  notifyPermission,
  notifySupported,
  notifyTest,
  requestNotifyPermission,
  type NotifyPermission,
} from '@/lib/notify';
import { AboutPane } from './about';
import { AppearancePane } from './appearance';
import type {
  AppConfig,
  CapabilityMatrix,
  FeatureMatrix,
  FeatureRow,
  HealthReport,
  RuleKind,
  Schedule,
  ScheduleAction,
  SelfcheckReport,
  SpaceEntry,
  SpaceReport,
} from '@/lib/types';

// 设置页分四个标签页，按「要不要动手」排序：需要用户操作的放前面，只需查看的放后面。
//   常规 / 自动任务 / 维护 = 改设置、管任务、清回收站；
//   关于 = 网络自检、已知限制、版本更新，从排障到查看逐层收尾。
type TabKey = 'general' | 'schedule' | 'maintenance' | 'about';

export default function SettingsPage() {
  const [tab, setTab] = useState<TabKey>('general');
  return (
    <div className="stagger flex flex-col gap-5">
      <TabBar tab={tab} onChange={setTab} />
      {tab === 'general' && (
        <>
          <AppearancePane />
          <PageSwitchPane />
          <GeneralPane />
          <FeaturesPane />
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
      {tab === 'about' && (
        <>
          {/* 顺序固定为 网络自检 → 已知限制 → 关于与更新：先排障，再看约束，最后才是版本 */}
          <SelfcheckPane />
          <LimitsPane />
          <AboutPane />
        </>
      )}
    </div>
  );
}

function TabBar({ tab, onChange }: { tab: TabKey; onChange: (t: TabKey) => void }) {
  const tabs: { key: TabKey; label: string; icon: LucideIcon }[] = [
    { key: 'general', label: '常规', icon: SlidersHorizontal },
    { key: 'schedule', label: '自动任务', icon: CalendarClock },
    { key: 'maintenance', label: '维护', icon: Wrench },
    { key: 'about', label: '关于', icon: Info },
  ];
  return (
    <Tabs
      label="设置分区"
      size="md"
      variant="solid"
      value={tab}
      onValueChange={(next) => onChange(next as TabKey)}
      className="self-start rounded-lg border border-line bg-card p-1 shadow-card"
      items={tabs.map((item) => {
        const Icon = item.icon;
        return { value: item.key, label: item.label, icon: <Icon size={14} /> };
      })}
    />
  );
}

function GeneralPane() {
  const [notifyOn, setNotifyOn] = useState(false);
  const [perm, setPerm] = useState<NotifyPermission>('unsupported');
  const [version, setVersion] = useState('');

  // 权限可能被用户在浏览器站点设置里改（页面本身收不到事件），
  // 所以每次窗口重新获得焦点都按真实权限校正一次开关显示。
  useEffect(() => {
    queryCmd('app_version').then(setVersion).catch(() => {});
    const sync = () => {
      setNotifyOn(notifyEnabled());
      setPerm(notifyPermission());
    };
    sync();
    window.addEventListener('focus', sync);
    document.addEventListener('visibilitychange', sync);
    return () => {
      window.removeEventListener('focus', sync);
      document.removeEventListener('visibilitychange', sync);
    };
  }, []);

  return (
    <Section
      title="通用"
      actions={version && <span className="text-xs text-muted">当前版本 v{version}</span>}
    >
      <div className="flex flex-col gap-4">
        <div>
          <label className="flex items-center gap-3 text-sm">
            <Switch
              // 开关状态以「浏览器权限 + 本地开关」双满足为准：
              // 权限没给时哪怕本地开着也发不出去，不能让开关骗人。
              checked={notifyOn && perm === 'granted'}
              disabled={!notifySupported()}
              label="桌面通知"
              onCheckedChange={async (next) => {
                if (!next) {
                  disableNotify();
                  setNotifyOn(false);
                  setPerm(notifyPermission());
                  toast('已关闭桌面通知');
                  return;
                }
                const permission = await requestNotifyPermission();
                setPerm(permission);
                if (permission === 'granted') {
                  const ok = await enableNotify();
                  setNotifyOn(ok);
                  toast('已开启桌面通知：标签页在后台时会提醒签到与活动轮询、登录结果、领取入账与凭据临期');
                } else if (permission === 'denied') {
                  setNotifyOn(false);
                  toast(
                    '浏览器拒绝了通知权限。点地址栏左侧的站点设置 → 通知 → 允许，再回来打开这个开关',
                  );
                } else if (permission === 'unsupported') {
                  setNotifyOn(false);
                  toast('当前页面不是安全上下文或不支持系统通知，无法开启');
                } else {
                  setNotifyOn(false);
                  toast('授权没有完成（可能关掉了授权框），再点一次开关即可重新申请');
                }
              }}
            />
            <span className="font-medium">桌面通知</span>
          </label>
          <p className="mt-1.5 pl-[26px] text-xs text-muted">
            {perm === 'unsupported'
              ? '此浏览器不支持系统通知'
              : perm === 'granted'
                ? '浏览器权限已允许 · 签到与活动轮询、登录结果、领取入账、凭据临期会弹系统通知（标签页在后台时）'
                : perm === 'denied'
                  ? '浏览器已拒绝通知权限：点地址栏左侧的站点设置 → 通知 → 允许，再回来打开开关'
                  : '还没授权：打开开关时浏览器会弹出授权框，点「允许」即可'}
          </p>
          {perm === 'granted' && (
            <p className="mt-1 pl-[26px] text-xs">
              <Tooltip content="授权后立即验证能不能真弹出来">
                <button
                  type="button"
                  className="cursor-pointer text-accent underline-offset-2 hover:underline"
                  onClick={() => {
                    const sent = notifyTest();
                    if (!notifyOn) {
                      toast('先打开上面的开关，再来试发（权限已允许，开关还没开）');
                      return;
                    }
                    toast(
                      sent
                        ? '已发出测试通知：没看到请检查 Windows 通知中心是否把浏览器静音了'
                        : '测试通知没有发出去：确认系统通知没被关闭',
                    );
                  }}
                >
                  发一条测试通知
                </button>
              </Tooltip>
            </p>
          )}
        </div>

      </div>
    </Section>
  );
}

/**
 * 「功能开关」：一级页面在这里统一开关，清单来自能力注册表——新加的同级页面
 * 自动出现在这里，不用再改一份名单。只影响导航入口与跨页跳转，不删数据、
 * 不卸载能力；`alwaysOn`（设置页）不提供关闭项。
 */
function PageSwitchPane() {
  const [hidden, setEnabled] = usePageToggles();
  const pages = useMemo(
    () => capabilities().filter((cap) => !cap.manifest.hidden && !cap.manifest.alwaysOn),
    [],
  );

  return (
    <Section title="功能开关">
      <div className="grid gap-x-8 gap-y-2 md:grid-cols-2">
        {pages.map((cap) => (
          <label
            key={cap.manifest.id}
            className="flex min-w-0 cursor-pointer items-center gap-3 text-sm"
          >
            <Switch
              checked={!hidden.has(cap.manifest.id)}
              onCheckedChange={(next) => setEnabled(cap.manifest.id, next)}
              label={cap.manifest.title}
              hint={cap.manifest.note}
            />
            <span className="truncate font-medium">{cap.manifest.title}</span>
            <span className="shrink-0 text-xs text-muted">{cap.manifest.group}</span>
          </label>
        ))}
      </div>
    </Section>
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
    <Section
      title="配置"
      note={
        <Hint content="并发实际执行收敛到 1-6，间隔上限 60 秒，越界值保存时按此修正">
          data\config.toml
        </Hint>
      }
    >
      <div className="grid gap-4 md:grid-cols-2">
        <div>
          <label className="flex items-center gap-4 text-sm">
            <Hint
              className="w-28 shrink-0 text-muted"
              content="同时跑几个账号的签到。调大更快，但同一平台短时间请求过多更容易被限流；网络差或常失败时调回 1-2。"
            >
              签到并发
            </Hint>
            <Picker
              label="签到并发"
              value={String(draft.checkin.max_concurrent)}
              onValueChange={(next) =>
                patch({ checkin: { ...draft.checkin, max_concurrent: numOr(next, 3) } })
              }
              options={[1, 2, 3, 4, 5, 6].map((n) => ({ value: String(n), label: `${n} 个` }))}
            />
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <Hint
              className="w-28 shrink-0 text-muted"
              content="两个账号之间等多久再发下一个。调大更稳、更不容易被限流，代价是一轮签到更慢；0 = 不等待。"
            >
              账号间隔
            </Hint>
            <Picker
              label="账号间隔"
              value={String(draft.checkin.gap_ms)}
              onValueChange={(next) =>
                patch({ checkin: { ...draft.checkin, gap_ms: numOr(next, 0) } })
              }
              options={[0, 500, 1000, 2000, 5000].map((ms) => ({
                value: String(ms),
                label: ms === 0 ? '不间隔' : `${ms / 1000} 秒`,
              }))}
            />
          </label>
        </div>
        <div>
          <label className="flex items-center gap-4 text-sm">
            <Hint
              className="w-28 shrink-0 text-muted"
              content="单次请求最多等多少秒。走代理或平台慢时调大，能少一些「超时」失败；调太小会把本来能成的请求判成失败。"
            >
              请求超时
            </Hint>
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
            <Hint
              className="w-28 shrink-0 text-muted"
              content="只读请求（查积分/查状态）失败后再试几次。调大能扛偶发抖动，但一轮耗时变长；写动作不重试，避免重复提交。"
            >
              GET 重试
            </Hint>
            <Picker
              label="GET 重试"
              value={String(draft.network.get_retry)}
              onValueChange={(next) =>
                patch({ network: { ...draft.network, get_retry: numOr(next, 1) } })
              }
              options={[0, 1, 2, 3].map((n) => ({ value: String(n), label: `${n} 次` }))}
            />
          </label>
        </div>
        {/* 用量采集、ZCode 领取、启动扫描属于后台循环，统一放到「自动任务 → 内置任务」里改 */}
        <div>
          <label className="flex items-center gap-4 text-sm">
            <Hint
              className="w-28 shrink-0 text-muted"
              content="每个账号在详情页保留多少条旅行/活动记录。只影响显示与本机存储，不影响平台数据。"
            >
              旅行记录条数
            </Hint>
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
        {/* 桌面时代的「关闭按钮 / 自启最小化」已随托盘退役：字段仍在配置里
            （兼容旧 config.toml），但不再提供界面入口——浏览器形态没有窗口可关。 */}
      </div>

      {/* 资料库采集的阈值都走这一份配置：改完保存即生效，不另写轮询。 */}
      <div className="mt-3 border-t border-line pt-3">
        <div className="mb-1 flex items-center gap-2 text-xs font-semibold text-fg2">
          资料库采集
          <Hint
            className="font-normal text-muted"
            content="采集只读公开元数据；分页预算与间隔决定一轮多久、覆盖多深，不改变数据源本身的覆盖范围"
          >
            数值越界会在保存时按允许范围修正
          </Hint>
        </div>
        <div className="grid gap-3 md:grid-cols-2">
          <label className="flex items-center gap-3 text-sm">
            <Switch
              checked={draft.catalog.enabled}
              onCheckedChange={(checked) => patch({ catalog: { ...draft.catalog, enabled: checked } })}
              label="自动采集"
              hint="关闭后不再自动采集；手动采集与已有资料不受影响"
            />
          </label>
          {([
            ['同步间隔', 'sync_interval_min', '分钟', '两轮自动采集之间等多久；调大更省请求，调小更新更勤'],
            ['全量检查', 'full_check_days', '天', '多久跑一次全量目录（含已完结作品）；日常增量只补近期更新'],
            ['历史天数', 'history_days', '天', '没填日期时，采集从今天往前覆盖多少天'],
            ['未来天数', 'future_days', '天', '没填日期时，采集向未来覆盖多少天'],
            ['每轮分页预算', 'max_pages_per_job', '页', '一轮最多翻多少页；用完后保留游标，下次从断点继续'],
            ['请求间隔', 'request_gap_ms', '毫秒', '两次请求之间等多久；调大更稳，调小更快但更容易被限流'],
            ['单源超时', 'source_timeout_secs', '秒', '一个数据源一轮最多跑多久；超时保留已完成的分页'],
            ['详情刷新', 'detail_interval_hours', '小时', 'Bangumi 等详情源多久补一次；太频繁只会重复请求'],
            ['重试次数', 'retry_count', '次', '单次请求失败后的重试上限；只重试可安全重放的 GET'],
            ['重试间隔', 'retry_delay_ms', '毫秒', '每次重试前等多久，按次数递增'],
          ] as const).map(([label, key, unit, hint]) => (
            <label key={key} className="flex items-center gap-3 text-sm">
              <Hint className="w-28 shrink-0 text-muted" content={hint}>{label}</Hint>
              <Input
                type="number"
                min={1}
                className="w-24"
                value={draft.catalog[key]}
                onChange={(event) =>
                  patch({ catalog: { ...draft.catalog, [key]: numOr(event.target.value, draft.catalog[key]) } })
                }
              />
              <span className="text-xs text-muted">{unit}</span>
            </label>
          ))}
        </div>
        <label className="mt-2 flex items-center gap-3 text-sm">
          <Hint
            className="w-28 shrink-0 text-muted"
            content="只影响 TVmaze 的国家电视日历；逗号分隔的大写两位地区码"
          >
            关注国家
          </Hint>
          <Input
            className="min-w-0 flex-1"
            value={draft.catalog.countries.join(', ')}
            onChange={(event) =>
              patch({
                catalog: {
                  ...draft.catalog,
                  countries: event.target.value
                    .split(/[,，\s]+/)
                    .map((code) => code.trim().toUpperCase())
                    .filter(Boolean),
                },
              })
            }
          />
        </label>
      </div>

      {/* CodeBuddy 两个客户端的数据目录：默认装在各软件缺省位置，改了安装/配置目录才填。
          留空 = 用缺省；决定账号详情里「切换 IDE / 切换插件」是否能点、写盘往哪写。 */}
      <div className="mt-3 border-t border-line pt-3">
        <div className="mb-1 text-xs font-semibold text-fg2">CodeBuddy 客户端目录（可选）</div>
        <div className="grid gap-4 md:grid-cols-2">
          <div>
            <label className="flex items-center gap-4 text-sm">
              <Hint
                className="w-28 shrink-0 text-muted"
                content={'CodeBuddy IDE 的数据目录（缺省 %APPDATA%\\CodeBuddy CN）。改了 user-data-dir / 安装目录才需要填，留空用缺省。'}
              >
                CodeBuddy IDE
              </Hint>
              <Input
                className="min-w-0 flex-1"
                value={draft.codebuddy.ide_dir}
                placeholder="%APPDATA%\CodeBuddy CN"
                onChange={(e) =>
                  patch({ codebuddy: { ...draft.codebuddy, ide_dir: e.target.value } })
                }
              />
            </label>
          </div>
          <div>
            <label className="flex items-center gap-4 text-sm">
              <Hint
                className="w-28 shrink-0 text-muted"
                content={'JetBrains 配置目录（缺省 %APPDATA%\\JetBrains，插件与 secret-storage.xml 所在）。改了 idea.config.path 才需要填，留空用缺省。'}
              >
                插件（IDEA）
              </Hint>
              <Input
                className="min-w-0 flex-1"
                value={draft.codebuddy.idea_dir}
                placeholder="%APPDATA%\JetBrains"
                onChange={(e) =>
                  patch({ codebuddy: { ...draft.codebuddy, idea_dir: e.target.value } })
                }
              />
            </label>
          </div>
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
              // config_set 是整份覆盖：功能开关、内置任务都在别的卡片里即时保存，
              // 这里以最新配置为底，只覆盖本卡片真正编辑的字段，避免把别处的改动静默写回。
              // 取不到就退回草稿——不能因为一次读取失败把保存整块打断。
              const latest = await queryCmd('config_get').catch(() => null);
              const base = latest ?? draft;
              const merged = {
                ...base,
                checkin: draft.checkin,
                network: draft.network,
                codebuddy: draft.codebuddy,
                activity: { ...base.activity, travel_events_limit: draft.activity.travel_events_limit },
              };
              if ((await runCmd('config_set', { config: merged })) === undefined) return;
              toast('配置已保存并生效（同时写入 data/config.toml）');
              await Promise.all([
                qc.invalidateQueries({ queryKey: ['config'] }),
                qc.invalidateQueries({ queryKey: ['platform-features'] }),
              ]);
            })
          }
        >
          保存配置
        </Button>
        <Button size="sm" variant="ghost" disabled={busy === 'config'} onClick={() => void queryCmd('config_get').then(setDraft)}>
          还原
        </Button>
      </div>
    </Section>
  );
}


/**
 * 「各软件功能」：哪个软件在界面上露出哪个入口，在这里勾，页面里不再写死。
 *
 * 两层判据都在后端：可用性来自 `app/capability.rs` 的能力表（平台本身做不到的格子
 * 只显示原因、不给勾——勾上也只是一次必然失败的操作），勾选状态即时写进
 * `data/config.toml` 的 `[features.*]`。这里改的是**界面入口**，不改变平台能力，
 * 也不会停掉已经排好的定时任务。
 */
function FeaturesPane() {
  const qc = useQueryClient();
  const { busy, run: runBusy } = useBusy();
  const matrix = useQuery<FeatureMatrix>({
    queryKey: ['platform-features'],
    queryFn: () => queryCmd('platform_features'),
  });
  const data = matrix.data;

  const toggle = (platform: string, feature: string, next: boolean) =>
    runBusy(`feature:${platform}:${feature}`, async () => {
      // config_set 是整份覆盖，先把当前配置读回来再改这一格。
      // 读不到就不写：拿半份配置覆盖会把别的设置一起冲掉。
      const cfg = await queryCmd('config_get').catch(() => null);
      if (!cfg) {
        toast('读不到当前配置，稍后再试一次');
        return;
      }
      const current = cfg.features ?? {};
      const saved = await runCmd('config_set', {
        config: {
          ...cfg,
          features: {
            ...current,
            [platform]: { ...(current[platform] ?? {}), [feature]: next },
          },
        },
      });
      if (saved === undefined) return;
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['platform-features'] }),
        qc.invalidateQueries({ queryKey: ['config'] }),
      ]);
    });

  const columns: Column<FeatureRow>[] = data
    ? [
        {
          key: 'platform',
          header: '软件',
          width: 180,
          cell: (row) => <span className="whitespace-nowrap font-medium">{row.label}</span>,
        },
        ...data.features.map((feature, index) => ({
          key: feature.key,
          header: (
            <Hint focusable={false} content={feature.hint}>
              {feature.label}
            </Hint>
          ),
          width: 116,
          cell: (row: FeatureRow) => {
            const cell = row.cells[index];
            // 平台本身没有的能力：给原因，不给勾
            if (!cell.available) {
              return (
                <Hint focusable={false} className="cursor-help text-xs text-muted" content={cell.note}>
                  平台不支持
                </Hint>
              );
            }
            return (
              <Switch
                checked={cell.enabled}
                disabled={!!busy}
                label={`${row.label}：${feature.label}`}
                hint={feature.hint}
                size="sm"
                onCheckedChange={(next) => toggle(row.key, feature.key, next)}
              />
            );
          },
        })),
      ]
    : [];

  return (
    <Section
      title={
        <Hint
          focusable={false}
          content="控制账号中心显示哪些入口；取消勾选不会删除凭据，也不影响已排好的定时任务。平台不支持的格子只给原因，不给勾。"
        >
          各软件功能
        </Hint>
      }
    >
      {!data && (
        <table className="w-full border-collapse">
          <tbody>
            <SkeletonRows rows={4} cols={5} />
          </tbody>
        </table>
      )}
      {data && (
        <div className="overflow-x-auto">
          <DataTable dense columns={columns} rows={data.platforms} rowKey={(row) => row.key} empty={null} />
        </div>
      )}
    </Section>
  );
}

/**
 * 「已知限制」：平台侧的硬约束，本应用改不了的，集中说清楚。
 * 文案真源在后端能力表（`app/capability.rs` 的 LIMITS），与矩阵同一处维护——
 * 账号中心的行为一变，限制文案跟着变，不会两边对不上。
 */
function LimitsPane() {
  const matrix = useQuery<CapabilityMatrix>({
    queryKey: ['capabilities'],
    queryFn: () => queryCmd('platform_capabilities'),
    staleTime: 60_000,
  });
  const limits = matrix.data?.limits ?? [];
  return (
    <Section title="已知限制">
      {matrix.isLoading ? (
        <SkeletonBlock rows={6} />
      ) : limits.length === 0 ? (
        <StateBlock
          kind="empty"
          title="没有读到限制清单"
          detail="能力表还没取回来，不代表这些限制不存在。"
          onRetry={() => void matrix.refetch()}
        />
      ) : (
        <div className="flex flex-col gap-1">
          {limits.map((item) => (
            <Tooltip
              key={`${item.scope}-${item.title}`}
              content={[item.detail, item.workaround && `怎么做：${item.workaround}`]
                .filter(Boolean)
                .join('\n')}
            >
              <div className="flex flex-wrap items-baseline gap-2 px-1 py-1.5">
                <span className="rounded bg-hover px-1.5 text-xs text-fg2">{item.scope}</span>
                <span className="text-sm">{item.title}</span>
              </div>
            </Tooltip>
          ))}
        </div>
      )}
    </Section>
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
    <Section
      title="网络自检"
      actions={
        <Button size="sm" variant="soft" loading={busy === 'selfcheck'} disabled={busy === 'selfcheck'} onClick={() =>
          runBusy('selfcheck', async () => {
            const report = await runCmd('network_selfcheck');
            if (report !== undefined) setReport(report);
          })
        }>
          {busy === 'selfcheck' ? '检测中…' : '运行自检'}
        </Button>
      }
    >
      {!report && (
        <p className="text-xs text-muted">
          国际版账号报「网络错误」时先跑这里：确认系统代理、三个国际端点与登录档案。
        </p>
      )}
      {report && (
        <div className="flex flex-col gap-1.5 text-xs">
          <div className="flex flex-wrap items-center gap-2">
            <Tag tone={report.proxy_enabled ? 'ok' : 'warn'}>
              {report.proxy_enabled ? '系统代理已开启' : '系统代理未开启'}
            </Tag>
            {report.proxy_server && (
              <Hint className="font-mono text-muted" content="WinINET ProxyServer">
                {report.proxy_server}
              </Hint>
            )}
            {!report.proxy_enabled && (
              <span className="text-muted">国际版直连必然超时——先开代理再测</span>
            )}
          </div>
          {report.endpoints.map((endpoint) => (
            <div key={endpoint.url} className="flex flex-wrap items-center gap-2">
              <Tag tone={endpoint.ok ? 'ok' : 'bad'}>{endpoint.ok ? '可达' : '不通'}</Tag>
              <Hint className="font-mono" content={endpoint.url}>
                {endpoint.url.replace('https://', '').replace(/\/$/, '')}
              </Hint>
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
    </Section>
  );
}

/**
 * 「运行与存储」：仪表盘只留「要不要动手」的结论，运行细节归设置页。
 * 数据目录逐项占用与健康报告合在一张卡，一个入口看全。
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
  const failed = health.isError || space.isError;

  return (
    <Section
      title="运行与存储"
      note={space.data ? `合计 ${fmtSize(space.data.total)}` : undefined}
      actions={
        <Button size="sm" onClick={() => void runCmd('maintenance_open_data')}>
          打开数据目录
        </Button>
      }
    >
      {/* 读失败必须显示原因：这里以前只画骨架/空列表，接口一报错就整块空白，看着像没数据 */}
      {failed && (
        <p className="text-xs text-bad">
          读取失败：
          {String((health.error ?? space.error)?.message ?? '未知错误')}
          <button
            type="button"
            className="ml-2 cursor-pointer underline underline-offset-2"
            onClick={() => {
              void health.refetch();
              void space.refetch();
            }}
          >
            重试
          </button>
        </p>
      )}

      {report && (
        <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
          <dt className="text-muted">数据目录</dt>
          <Tooltip content={report.data_dir}>
            <dd className="truncate font-mono">{report.data_dir}</dd>
          </Tooltip>
          <dt className="text-muted">平台</dt>
          <dd className="flex flex-wrap gap-1">
            {report.platforms.map((platform) => (
              <Hint
                key={platform.platform}
                className={
                  platform.failing > 0
                    ? 'rounded bg-bad/15 px-1 text-bad'
                    : 'rounded bg-hover px-1 text-fg2'
                }
                content={`最近成功：${fmtTime(platform.last_ok_at)} · 最近失败：${fmtTime(platform.last_fail_at)}`}
              >
                {platform.label} {platform.accounts}
                {platform.failing > 0 ? ` · 失败 ${platform.failing}` : ''}
              </Hint>
            ))}
          </dd>
        </dl>
      )}

      <div className="mt-2 border-t border-line pt-1">
        {space.data && (
          <p className="py-1 text-xs text-muted">
            数据目录：<span className="font-mono text-fg2">{space.data.root}</span>
          </p>
        )}
        {space.isLoading && <SkeletonBlock rows={3} />}
        {space.data && space.data.entries.length === 0 && (
          <p className="py-1 text-xs text-muted">数据目录还没有可统计的内容。</p>
        )}
        {(space.data?.entries ?? []).map((entry) => (
          <div
            key={entry.name}
            className="flex items-center gap-2 border-b border-line px-1 py-1 text-sm last:border-b-0"
          >
            <Hint className="flex-1 truncate" content={entry.path}>
              {entry.name}
            </Hint>
            <span className="tabular-nums text-muted">{fmtSize(entry.size)}</span>
          </div>
        ))}
      </div>
    </Section>
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
        filter: { source: null, account: null, project: null, state: 'archived', q: null, origin: null, unnamed: false, pinned: false, hidden: false, body: null, tags: [], sort: 'last_at' },
      }),
  });
  const total = archived.data?.total ?? 0;
  return (
    <Section
      title="已归档会话"
      note={archived.isLoading ? '统计中…' : total > 0 ? `已归档 ${total} 条` : undefined}
      actions={
        <Tooltip content="归档只写本地标记、不删文件；可预览、取消归档或彻底删除。自动归档按最后活动时间计算，超过设定天数没动过的会话会被收进来。">
          <Button
            size="sm"
            variant="soft"
            onClick={() =>
              window.dispatchEvent(new CustomEvent('aim:navigate', { detail: 'sessions-archive' }))
            }
          >
            打开已归档会话
          </Button>
        </Tooltip>
      }
    />
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
    <div className="flex min-w-0 flex-col gap-5">
      {/* 摘要行：批次明细与逐批删除收进对话框，设置页不再被回收站列表撑高 */}
      <Section
        title={
          <Hint
            focusable={false}
            content="移除平台技能时，原文件副本先进这里，确认没问题再清空。会话删除不进回收站——那条路径不可恢复，删除前会单独确认。"
          >
            平台技能回收站
          </Hint>
        }
        note={
          trash.isError
            ? '读取失败'
            : (trash.data ?? []).length > 0
              ? `${(trash.data ?? []).length} 个批次 · 共 ${fmtSize((trash.data ?? []).reduce((sum, batch) => sum + batch.size, 0))}`
              : '空的'
        }
        actions={
          (trash.data ?? []).length > 0 && (
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
          )
        }
      />

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
            <Hint className="flex-1 truncate font-mono text-xs" content={batch.path}>{batch.name}</Hint>
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
          // next_at 只由后端按平台状态写入：这里传 null，编辑规则时也会被后端清掉
          ...(editing ?? {
            id: '',
            name: '',
            enabled: true,
            last_run: null,
            next_at: null,
            account_iid: null,
            created_at: null,
          }),
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

  // 账号状态刷新：云端校验凭据有效性 + 校正到期时间（不重扫入库）。
  // 与账号中心「刷新」（重扫客户端、写回最新凭据）分开，这里给一个不用跳页面的入口。
  const refreshStatusNow = () =>
    runBusy('status-now', async () => {
      const report = await runCmd('account_refresh_all');
      if (report === undefined) return;
      toast(
        `状态已刷新：${report.total} 个账号 · 云端确认 ${report.cloud_ok} · 回退本机 ${report.cloud_fallback} · 已过期 ${report.expired}`,
      );
      qc.invalidateQueries({ queryKey: ['accounts'] });
      refresh();
    });

  return (
    <>
    <BuiltinCard />
    <Section
      title={
        <Hint
          focusable={false}
          content="同一任务不会叠着跑（上一轮没结束就跳过）；不同任务同时到点时并行，建议错开时刻。规则改完即时生效。猫猫旅行在到站后 5 分钟内自动领取并续派下一趟。"
        >
          定时任务
        </Hint>
      }
      actions={
        <Tooltip content="云端校验凭据有效性并校正到期时间（不重扫客户端）">
          <Button
            size="sm"
            loading={busy === 'status-now'}
            disabled={!!busy}
            onClick={refreshStatusNow}
          >
            立即刷新账号状态
          </Button>
        </Tooltip>
      }
    >

      {list.isLoading && <SkeletonBlock rows={3} />}
      {(list.data ?? []).length === 0 && <p className="text-sm text-muted">还没有定时任务。</p>}

      {(list.data ?? []).map((item) => (
        <div
          key={item.id}
          className="flex flex-wrap items-center gap-2 border-b border-line px-1 py-1.5 text-sm last:border-b-0"
        >
          <Hint className="w-48 max-w-full truncate font-medium" content={item.name}>
            {item.name}
          </Hint>
          <span className="w-28 text-xs text-muted">{ruleText(item)}</span>
          <span className="w-24 text-xs text-muted">上次 {shortTs(item.last_run)}</span>
          {item.next_at && (
            <Hint className="text-xs text-accent" content="平台给出的预计到站时刻 + 5 分钟领取冗余">
              下次领取 ~ {shortTs(item.next_at)}
            </Hint>
          )}
          <span className="flex-1" />
          <label className="flex cursor-pointer items-center gap-1 text-xs text-fg2">
            <Switch
              checked={item.enabled}
              disabled={!!busy}
              label={`启用 ${item.name}`}
              size="sm"
              onCheckedChange={() => toggle(item)}
            />
            启用
          </label>
          <Button size="sm" disabled={!!busy} onClick={() => edit(item)}>
            编辑
          </Button>
          <Tooltip content="按这条任务现在跑一次（记一条手动执行动作）">
            <Button
              size="sm"
              loading={busy === `schedule-run:${item.id}`}
              disabled={!!busy}
              onClick={() => runNow(item)}
            >
              立即执行
            </Button>
          </Tooltip>
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

      {/* 平台差异必须写在用户配间隔的地方：全局间隔只是上限，接口性质不同的平台会自己放慢 */}
      <RefreshPolicyNote />

      <div className="mt-2 flex flex-wrap items-center gap-2">
        {editing && <span className="text-xs font-semibold">编辑：{editing.name}</span>}
        <Input
          className="w-40"
          aria-label="任务名称"
          value={name}
          placeholder="任务名称（可留空自动命名）"
          onChange={(e) => setName(e.target.value)}
        />
        <Picker
          label="任务动作"
          value={action}
          disabled={!!editing}
          onValueChange={(next) => setAction(next as ScheduleAction)}
          options={[
            { value: 'checkin_round', label: '签到轮询' },
            { value: 'credits_round', label: '积分刷新' },
            { value: 'status_round', label: '账号状态刷新' },
            { value: 'catalog_sync', label: '资料库采集' },
            { value: 'travel_round', label: '猫猫旅行' },
          ]}
        />
        <Picker
          label="执行频率"
          value={kind}
          disabled={!!busy}
          onValueChange={(next) => setKind(next as RuleKind)}
          options={[
            { value: 'daily', label: '每天' },
            { value: 'every', label: '每隔' },
          ]}
        />
        {kind === 'daily' ? (
          // 原生时间控件：产出恒为 HH:mm，后端不再需要挡格式错误
          <Tooltip content="每天触发的时刻">
            <Input
              className="w-28"
              type="time"
              aria-label="触发时刻"
              value={at}
              onChange={(e) => setAt(e.target.value)}
            />
          </Tooltip>
        ) : (
          <>
            <Tooltip content={unit === 'hour' ? '间隔小时数（1–168）' : '间隔分钟数（1–10080）'}>
              <Input
                className="w-20"
                type="number"
                aria-label="间隔数值"
                min={1}
                max={unit === 'hour' ? 168 : 10080}
                value={every}
                onChange={(e) => setEvery(e.target.value)}
              />
            </Tooltip>
            <Picker
              label="间隔单位"
              value={unit}
              disabled={!!busy}
              onValueChange={(next) => setUnit(next as 'minute' | 'hour')}
              options={[
                { value: 'minute', label: '分钟' },
                { value: 'hour', label: '小时' },
              ]}
            />
          </>
        )}
        <Button size="sm" variant="primary" loading={busy === 'schedule-add'} disabled={!!busy} onClick={add}>
          {editing ? '保存' : '添加'}
        </Button>
        {editing && <Button size="sm" variant="ghost" disabled={!!busy} onClick={resetForm}>取消</Button>}
      </div>
    </Section>
    </>
  );
}

/**
 * 内置任务：代码里固定的后台循环。以前这些节奏写死在后台，只能改文件，
 * 现在摊到界面上：每一项即时写回 config.toml，下一轮就按新值走。
 */
function BuiltinCard() {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const cfg = useQuery<AppConfig>({
    queryKey: ['config'],
    queryFn: () => queryCmd('config_get'),
  });
  const data = cfg.data;

  // 单项即时保存：先拿最新配置再改一个字段，避免把别处刚保存的值覆盖掉
  const patch = (mutate: (config: AppConfig) => AppConfig) =>
    run('builtin', async () => {
      const latest = await queryCmd('config_get');
      if ((await runCmd('config_set', { config: mutate(latest) })) === undefined) return;
      toast('已生效');
      await qc.invalidateQueries({ queryKey: ['config'] });
    });

  const reset = () =>
    run('builtin-reset', async () => {
      const next = await runCmd('config_reset_builtin');
      if (next === undefined) return;
      toast('内置任务已恢复默认');
      qc.setQueryData(['config'], next);
    });

  const minuteInput = (
    value: number,
    onChange: (n: number) => void,
    label: string,
    title: string,
  ) => (
    <Tooltip content={title}>
      <Input
        type="number"
        min={1}
        max={1440}
        className="w-20"
        aria-label={label}
        value={value}
        disabled={!!busy || !data}
        onChange={(e) => onChange(numOr(e.target.value, value))}
      />
    </Tooltip>
  );

  return (
    <Section
      title="内置任务"
      actions={
        <Button size="sm" variant="soft" loading={busy === 'builtin-reset'} disabled={!!busy} onClick={reset}>
          恢复默认
        </Button>
      }
    >
      {!data && <SkeletonBlock rows={3} />}
      {data && (
        <div className="flex flex-col gap-2 text-sm">
          <div className="flex flex-wrap items-center gap-3">
            <Hint className="w-40 shrink-0 text-muted" content="本机会话索引多久增量扫一遍；改小能更快看到新会话">
              会话扫描
            </Hint>
            <span className="text-xs text-muted">每</span>
            {minuteInput(
              data.sessions.scan_interval_min,
              (n) => patch((c) => ({ ...c, sessions: { ...c.sessions, scan_interval_min: n } })),
              '会话扫描间隔',
              '1-1440 分钟',
            )}
            <span className="text-xs text-muted">分钟</span>
          </div>

          <div className="flex flex-wrap items-center gap-3">
            <Hint className="w-40 shrink-0 text-muted" content="后台多久扫一次各账号用量；只读本地文件">
              用量采集
            </Hint>
            <span className="text-xs text-muted">每</span>
            {minuteInput(
              data.usage.scan_interval_min,
              (n) => patch((c) => ({ ...c, usage: { ...c.usage, scan_interval_min: n } })),
              '用量采集间隔',
              '1-1440 分钟',
            )}
            <span className="text-xs text-muted">分钟</span>
          </div>

          <div className="flex flex-wrap items-center gap-3">
            <span className="w-40 shrink-0 text-muted">ZCode 自动领取</span>
            <Switch
              checked={data.activity.zcode_auto_claim}
              disabled={!!busy}
              label="ZCode 自动领取"
              hint="开启后按间隔探测可领奖励；领取要过浏览器验证码"
              onCheckedChange={(next) =>
                patch((c) => ({
                  ...c,
                  activity: { ...c.activity, zcode_auto_claim: next },
                }))
              }
            />
            <span className="text-xs text-muted">每</span>
            {minuteInput(
              data.activity.zcode_claim_min,
              (n) => patch((c) => ({ ...c, activity: { ...c.activity, zcode_claim_min: n } })),
              'ZCode 领取间隔',
              '1-1440 分钟',
            )}
            <span className="text-xs text-muted">分钟探测一轮</span>
          </div>

          <div className="flex flex-wrap items-center gap-3">
            <span className="w-40 shrink-0 text-muted">启动扫描客户端</span>
            <Switch
              checked={data.accounts.scan_on_startup}
              disabled={!!busy}
              label="启动扫描客户端"
              hint="启动时把本地客户端里的新账号捞进账号库；关掉则只从浏览器登录添加"
              onCheckedChange={(next) =>
                patch((c) => ({
                  ...c,
                  accounts: { ...c.accounts, scan_on_startup: next },
                }))
              }
            />
            <span className="text-xs text-muted">
              {data.accounts.scan_on_startup ? '启动时扫描本地登录态' : '只从浏览器登录添加'}
            </span>
          </div>
        </div>
      )}
    </Section>
  );
}

/**
 * 各平台的自动刷新节奏。
 *
 * 数据来自 Provider 自己（`credits_min_interval_minutes` / `credits_interval_note`），
 * 与调度器读同一份——前端不写死「豆包 30 分钟」，也不替平台解释原因。
 */
function RefreshPolicyNote() {
  const matrix = useQuery<CapabilityMatrix>({
    queryKey: ['capabilities'],
    queryFn: () => queryCmd('platform_capabilities'),
    staleTime: Infinity,
  });
  const policies = matrix.data?.refresh_policies ?? [];
  if (policies.length === 0) return null;
  const slow = policies.filter((p) => p.min_minutes > 0);
  const followers = policies.length - slow.length;

  return (
    <div className="mt-3 px-3 py-2">
      <Tooltip content="上面的间隔是上限：到点这一轮只查「距上次成功查询已超过平台最小间隔」的账号，没到点的直接跳过——这是正常的限速，不是失败。手动点账号中心的「签到 · 积分」不受限制。">
        <p className="text-xs font-semibold">
          各平台的自动刷新节奏
        </p>
      </Tooltip>
      <ul className="mt-1.5 flex flex-col gap-1 text-xs">
        {slow.map((p) => (
          <li key={p.platform} className="flex flex-wrap gap-x-2">
            <span className="w-36 shrink-0 text-fg2">{p.label}</span>
            <span className="text-fg2">最快 {p.min_minutes} 分钟一次</span>
            {p.note && <span className="text-muted">· {p.note}</span>}
          </li>
        ))}
        {followers > 0 && (
          <li className="flex flex-wrap gap-x-2">
            <span className="w-36 shrink-0 text-fg2">其余 {followers} 家</span>
            <span className="text-muted">跟随任务间隔（余额接口，可按分钟级刷新）</span>
          </li>
        )}
      </ul>
    </div>
  );
}
