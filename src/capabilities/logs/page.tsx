// 日志：默认只看最近 10 分钟，异常单独成视图，点一行看上下文，异常详情用弹窗展开。
//
// 三块内容都来自同一份 app.jsonl，只是看的角度不同：
//   - 全部日志：最近 10 分钟的运行流水（成功行仍默认折叠，避免刷屏）；
//   - 异常：只留 warn/error，信息级噪声全部不显示；
//   - 执行动作：只筛 module == "action" 的动作行。
// 时间窗是「看多近」的唯一开关：默认 10 分钟，超过就隐藏；回看旧日志改时间窗或自定义日期。
// 上下文按文件物理行号取（后端 `log_context`），前后行数由用户在现场决定。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { queryCmd, runCmd } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { Button, cn } from '@/components/ui/button';
import { confirmAction } from '@/components/ui/confirm';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, Input, Select, SkeletonBlock, SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { Tag } from '@/components/ui';
import { fmtAmount, fmtMs, fmtTime } from '@/lib/format';
import { takePreset } from '@/lib/nav';
import { ALERT_SEEN_KEY, newestAlertAt, shouldAlert } from './alerts';
import type {
  Level,
  LogContext,
  LogFilter,
  LogLine,
  LogSpan,
  LogStat,
  RecentClaim,
} from '@/lib/types';

type View = 'all' | 'alert' | 'action' | 'stat' | 'claims';
type WindowKey = '10m' | '30m' | '1h' | 'today' | '7d' | 'all' | 'custom';

const LEVEL_RANK: Record<Level, number> = { debug: 0, info: 1, warn: 2, error: 3 };

const TRIGGER_LABEL: Record<string, string> = {
  user: '手动',
  auto: '自动',
  schedule: '定时',
  startup: '启动',
  tray: '托盘',
};

const OUTCOME: Record<string, { text: string; className: string }> = {
  ok: { text: '成功', className: 'text-ok' },
  skip: { text: '跳过', className: 'text-muted' },
  fail: { text: '失败', className: 'text-bad' },
  // 网络不通 / 平台限流：不是账号坏了，别用失败色去催人处理
  retry: { text: '待重试', className: 'text-warn' },
  partial: { text: '部分成功', className: 'text-warn' },
};

const WINDOWS: { key: WindowKey; label: string }[] = [
  { key: '10m', label: '10 分钟' },
  { key: '30m', label: '30 分钟' },
  { key: '1h', label: '1 小时' },
  { key: 'today', label: '今天' },
  { key: '7d', label: '近 7 天' },
  { key: 'all', label: '全部' },
  { key: 'custom', label: '自定义' },
];

export default function LogsPage() {
  return <LogsPane />;
}

type Anchor = { anchor: number | null; ts: string; before: number; after: number };

function LogsPane() {
  const qc = useQueryClient();
  const [view, setView] = useState<View>('all');
  const [minLevel, setMinLevel] = useState<Level | null>(null);
  const [module, setModule] = useState('');
  const [keyword, setKeyword] = useState('');
  const [outcome, setOutcome] = useState('');
  const [trigger, setTrigger] = useState('');
  const [subject, setSubject] = useState('');
  // 时间窗：默认最近 10 分钟。相对窗口靠 tick 推进，跨过 10 分钟的行会自动掉出列表。
  const [windowKey, setWindowKey] = useState<WindowKey>('10m');
  const [fromDay, setFromDay] = useState<string | null>(null);
  const [toDay, setToDay] = useState<string | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [limit, setLimit] = useState(300);
  const [exporting, setExporting] = useState(false);
  const [clearing, setClearing] = useState(false);
  // 提示水位 = 上次看过异常时窗口里最新那条错误的时间（毫秒）。
  const [seenAt, setSeenAt] = useState<number | null>(null);
  const seenRef = useRef(0);
  // 成功日志默认折叠，点开才显示
  const [showOk, setShowOk] = useState(false);
  // 上下文 / 异常详情：两者都是弹窗，同一时刻只开一个
  const [contextAnchor, setContextAnchor] = useState<Anchor | null>(null);
  const [detailLine, setDetailLine] = useState<LogLine | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const tailRef = useRef<HTMLDivElement | null>(null);
  // 贴底才自动跟随：用户向上翻历史时，新日志不许把视口拽走
  const nearBottom = useRef(true);

  // 相对时间窗每 30 秒推进一次：列表里的「最近 10 分钟」不会因为一直不刷新而停在进页那一刻
  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  // 从接口反代等页面跳进来时带一次性预设（模块/关键词），读完即删
  useEffect(() => {
    const preset = takePreset<{ module?: string; keyword?: string; view?: View }>('logs');
    if (!preset) return;
    if (preset.module) setModule(preset.module);
    if (preset.keyword) setKeyword(preset.keyword);
    if (preset.view) setView(preset.view);
    // 外部跳转通常是要查某个模块的历史，10 分钟窗口太窄
    if (preset.module || preset.keyword) setWindowKey('today');
  }, []);

  const today = todayIso(new Date(nowMs));
  const dayFrom = fromDay ?? today;
  const dayTo = toDay ?? today;
  const relative = windowKey === '10m' || windowKey === '30m' || windowKey === '1h';

  const windowFrom = useMemo(() => {
    if (windowKey === '10m') return new Date(nowMs - 10 * 60_000).toISOString();
    if (windowKey === '30m') return new Date(nowMs - 30 * 60_000).toISOString();
    if (windowKey === '1h') return new Date(nowMs - 60 * 60_000).toISOString();
    if (windowKey === 'today') return dayStartIso(today);
    if (windowKey === '7d') return dayStartIso(shiftDay(today, -6));
    if (windowKey === 'custom') return dayStartIso(dayFrom);
    return null;
  }, [windowKey, nowMs, today, dayFrom]);
  const windowTo = useMemo(() => {
    if (windowKey === 'today') return dayEndIso(today);
    if (windowKey === '7d') return dayEndIso(today);
    if (windowKey === 'custom') return dayEndIso(dayTo);
    return null;
  }, [windowKey, today, dayTo]);
  const windowLabel =
    windowKey === 'custom'
      ? `${dayFrom} ~ ${dayTo}`
      : (WINDOWS.find((item) => item.key === windowKey)?.label ?? '');
  const defaultWindow = windowKey === '10m';

  // 可选范围 = 现存日志的首尾。日志按大小轮转（没有固定保留天数），
  // 写死「最近 N 天」会出现「选了却一条没有」的空范围，所以上界现算。
  const span = useQuery<LogSpan>({
    queryKey: ['log-span'],
    queryFn: () => queryCmd('log_span'),
    retry: false,
  });
  const earliestDay = span.data?.earliest ? dayOf(span.data.earliest) : undefined;

  // 起止互斥：起晚于止时把另一端拉过来，避免出现「空区间 + 一句没有日志」
  const changeFrom = (value: string) => {
    setFromDay(value || null);
    if (value && value > dayTo) setToDay(value);
  };
  const changeTo = (value: string) => {
    setToDay(value || null);
    if (value && value < dayFrom) setFromDay(value);
  };

  const filter: LogFilter = {
    level: null,
    // 异常视图用最低级别表达：一次覆盖 warn + error，info/debug 的噪声不进来
    min_level: view === 'alert' ? 'warn' : minLevel,
    module: module || null,
    keyword: keyword || null,
    subject: subject || null,
    action_only: view === 'action' ? true : null,
    outcome: view === 'action' ? outcome || null : null,
    trigger: view === 'action' ? trigger || null : null,
    from: windowFrom,
    to: windowTo,
    hide_legacy: view === 'action' || view === 'alert' ? true : null,
    limit,
  };

  // 空态要说清是「这 10 分钟没写」还是「所选范围没有」：默认窗口下只说「没有日志」，
  // 用户会以为日志丢了，其实是超过 10 分钟的日志被默认隐藏了。
  const emptyText =
    view === 'alert'
      ? `最近 ${windowLabel} 没有异常日志。`
      : view === 'action'
        ? defaultWindow
          ? '最近 10 分钟没有执行动作。要看更早的记录，把上方时间窗改成「今天」或更长。'
          : '所选时间范围内没有执行动作。'
        : defaultWindow
          ? '最近 10 分钟没有日志。更早的日志已按时间窗隐藏，想看就把时间窗改大。'
          : '所选时间范围内没有日志。';

  const query = useQuery<LogLine[]>({
    queryKey: ['log', JSON.stringify(filter)],
    queryFn: () => queryCmd('log_query', { filter }),
  });
  const groups = useQuery<LogStat[]>({
    queryKey: ['log-stats', JSON.stringify(filter)],
    queryFn: () => queryCmd('log_stats', { filter }),
    enabled: view === 'stat',
  });
  const live = useQuery<LogLine[]>({
    queryKey: ['log-live'],
    queryFn: () => [],
    staleTime: Infinity,
  });
  // 最近领取从仪表盘迁来：它本来就是「已经发生过的事」，与日志同源同视角
  const claims = useQuery<RecentClaim[]>({
    queryKey: ['recent-claims'],
    queryFn: () => queryCmd('credit_claims_recent', { limit: 50 }),
    enabled: view === 'claims',
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 60_000),
  });

  const rows = useMemo(() => {
    const liveRows = (live.data ?? []).filter((line) => matchFilter(line, filter));
    const seen = new Set<string>();
    return [...(query.data ?? []), ...liveRows]
      // 服务端已按时间窗筛过；这里再筛一遍是为了「跨过 10 分钟那一刻」不再显示旧行——
      // 查询结果要等下一次请求回来，界面不能在这段空档里假装它还新鲜
      .filter((line) => matchFilter(line, filter))
      .filter((line) => view !== 'alert' || line.level === 'warn' || line.level === 'error')
      .filter((line) => {
        // repeat 也要进 key：首行与合并行内容相同，只差次数
        const key = `${line.ts}|${line.module}|${line.action ?? ''}|${line.repeat ?? ''}|${line.message}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      })
      .sort((a, b) => a.ts.localeCompare(b.ts));
  }, [query.data, live.data, JSON.stringify(filter), view]);

  const stats = useMemo(() => {
    // 合并行代表 N 次真实发生：统计要按次数算，否则「异常 3」会漏掉被合并掉的 97 次
    const weight = (line: LogLine) => line.repeat ?? 1;
    const sum = (pick: (line: LogLine) => boolean) =>
      rows.filter(pick).reduce((n, line) => n + weight(line), 0);
    return {
      total: rows.length,
      abnormal: sum((line) => line.level === 'warn' || line.level === 'error'),
      failed: sum((line) => effectiveOutcome(line) === 'fail'),
      skipped: sum((line) => effectiveOutcome(line) === 'skip'),
    };
  }, [rows]);

  // 类型分窗：成功行默认折叠；主列表只留其余行。
  // 用户在执行动作里显式筛「成功」时不折叠——那是用户点名要看的内容。
  const isOkRow = (line: LogLine) =>
    line.level !== 'warn' && line.level !== 'error' && effectiveOutcome(line) === 'ok';
  const collapseOk = view !== 'alert' && !(view === 'action' && outcome === 'ok');
  const okRows = collapseOk ? rows.filter(isOkRow) : [];
  const mainRows = collapseOk ? rows.filter((line) => !isOkRow(line)) : rows;
  const alertRows = useMemo(
    () => rows.filter((line) => line.level === 'warn' || line.level === 'error'),
    [rows],
  );
  const errorCount = useMemo(
    () =>
      alertRows
        .filter((line) => line.level === 'error')
        .reduce((n, line) => n + (line.repeat ?? 1), 0),
    [alertRows],
  );
  const warnCount = useMemo(
    () =>
      alertRows
        .filter((line) => line.level === 'warn')
        .reduce((n, line) => n + (line.repeat ?? 1), 0),
    [alertRows],
  );
  const hasNewAlert =
    seenAt !== null && view !== 'alert' && view !== 'claims' && shouldAlert(alertRows, seenRef.current);

  // 水位从 kv 读回：读失败按 0 处理（宁可多提示一次，也不能把错误吞掉）
  useEffect(() => {
    queryCmd('settings_get', { key: ALERT_SEEN_KEY })
      .then((value) => {
        const at = Number(value);
        const saved = Number.isFinite(at) && at > 0 ? at : 0;
        // 读取期间用户可能已经点过「查看异常」：水位只许前进，不许被旧值覆盖
        seenRef.current = Math.max(seenRef.current, saved);
        setSeenAt((current) => (current === null ? saved : Math.max(current, saved)));
      })
      .catch(() => {
        setSeenAt((current) => current ?? 0);
      });
  }, []);

  const markSeen = useCallback((at: number) => {
    if (!Number.isFinite(at) || at <= seenRef.current) return;
    seenRef.current = at;
    setSeenAt(at);
    // 写失败只影响下次启动的起点，本会话仍以内存里的水位为准
    void queryCmd('settings_set', { key: ALERT_SEEN_KEY, value: at }).catch(() => {});
  }, []);

  /** 切到异常视图即视为「这批看过了」：返回列表后不再重复提示。 */
  const openAlerts = () => {
    setView('alert');
    setOutcome('');
    setTrigger('');
    setSubject('');
    markSeen(newestAlertAt(alertRows));
  };

  useEffect(() => {
    if (nearBottom.current) {
      tailRef.current?.scrollIntoView({ block: 'end' });
    }
  }, [rows.length]);

  const openContext = (line: LogLine, before = 20) =>
    setContextAnchor({ anchor: line.seq ?? null, ts: line.ts, before, after: before });

  return (
    <div className="flex h-[calc(100vh-7.5rem)] flex-col gap-2">
      {/* 工具栏：视图 / 时间窗 / 筛选 / 计数。放在滚动容器外面——筛选条件不该跟着日志滚走 */}
      <div className="rounded-lg border border-line bg-card">
        <div className="flex flex-wrap items-center gap-2 px-2 py-1.5">
          <div className="flex items-center gap-1 rounded-md border border-line p-0.5">
            <Button
              size="sm"
              variant={view === 'all' ? 'primary' : 'ghost'}
              onClick={() => {
                setView('all');
                setOutcome('');
                setTrigger('');
                setSubject('');
              }}
            >
              全部日志
            </Button>
            <Button
              size="sm"
              variant={view === 'alert' ? 'primary' : 'ghost'}
              onClick={openAlerts}
              title="只显示 warn / error，信息级噪声全部隐藏"
            >
              异常 {stats.abnormal}
            </Button>
            <Button
              size="sm"
              variant={view === 'action' ? 'primary' : 'ghost'}
              onClick={() => setView('action')}
            >
              执行动作
            </Button>
            <Button
              size="sm"
              variant={view === 'stat' ? 'primary' : 'ghost'}
              onClick={() => setView('stat')}
              title="按「模块 + 动作 + 结果」聚合，回答哪一类在反复出问题"
            >
              聚合
            </Button>
            <Button
              size="sm"
              variant={view === 'claims' ? 'primary' : 'ghost'}
              onClick={() => setView('claims')}
              title="本工具实际领取过的积分（签到 / 活动 / 猫猫旅行）"
            >
              最近领取
            </Button>
          </div>
          {view !== 'claims' && (
            <>
              {view !== 'alert' && (
                <Select
                  value={minLevel ?? ''}
                  title="只看该级别及以上"
                  onChange={(e) => setMinLevel((e.target.value || null) as Level | null)}
                >
                  <option value="">全部级别</option>
                  <option value="debug">debug+</option>
                  <option value="info">info+</option>
                  <option value="warn">warn+</option>
                  <option value="error">error</option>
                </Select>
              )}
              <Input
                placeholder="模块"
                className="w-24"
                value={module}
                onChange={(e) => setModule(e.target.value)}
              />
              <Input
                placeholder="关键词 / 证据"
                className="w-36"
                value={keyword}
                onChange={(e) => setKeyword(e.target.value)}
              />
              <Select value={String(limit)} onChange={(e) => setLimit(Number(e.target.value) || 300)}>
                <option value="100">最近 100 行</option>
                <option value="300">最近 300 行</option>
                <option value="1000">最近 1000 行</option>
              </Select>
              <span className="flex-1" />
              <Button
                size="sm"
                loading={clearing}
                onClick={async () => {
                  const ok = await confirmAction({
                    title: '清空日志',
                    message:
                      '清空当前 app.log / app.jsonl 与历史轮转文件？\n只删日志，不动账号、会话与积分数据。',
                    danger: true,
                  });
                  if (!ok) return;
                  setClearing(true);
                  try {
                    if ((await runCmd('log_clear')) === undefined) return;
                    qc.setQueryData(['log-live'], []);
                    await qc.invalidateQueries({ queryKey: ['log'] });
                    await qc.invalidateQueries({ queryKey: ['log-span'] });
                    setContextAnchor(null);
                    setDetailLine(null);
                    // 清空后旧行即使还留在内存里也不该再弹：水位推到「现在」
                    markSeen(Date.now());
                    toast('日志已清空');
                  } finally {
                    setClearing(false);
                  }
                }}
              >
                清空
              </Button>
              <Button
                size="sm"
                loading={exporting}
                title="导出当前筛选结果（时间窗同样生效）"
                onClick={async () => {
                  setExporting(true);
                  try {
                    toast(await queryCmd('log_export', { path: null, filter }));
                  } catch (e) {
                    toast(errText(e));
                  } finally {
                    setExporting(false);
                  }
                }}
              >
                导出
              </Button>
            </>
          )}
        </div>

        {view !== 'claims' && (
          <div className="flex flex-wrap items-center gap-2 border-t border-line/60 px-2 py-1">
            <span className="text-xs text-muted">时间窗</span>
            <span className="flex flex-wrap items-center gap-0.5 rounded border border-line p-0.5">
              {WINDOWS.map((item) => (
                <button
                  key={item.key}
                  type="button"
                  className={cn(
                    'cursor-pointer rounded px-1.5 py-0.5 text-xs',
                    windowKey === item.key ? 'bg-accent text-on-accent' : 'text-muted hover:text-fg',
                  )}
                  onClick={() => setWindowKey(item.key)}
                >
                  {item.label}
                </button>
              ))}
            </span>
            {windowKey === 'custom' && (
              <label className="flex items-center gap-1 text-xs text-muted">
                <Input
                  type="date"
                  className="w-[9.5rem] text-xs"
                  value={dayFrom}
                  min={earliestDay}
                  max={dayTo}
                  onChange={(e) => changeFrom(e.target.value)}
                />
                ~
                <Input
                  type="date"
                  className="w-[9.5rem] text-xs"
                  value={dayTo}
                  min={dayFrom}
                  max={today}
                  onChange={(e) => changeTo(e.target.value)}
                />
              </label>
            )}
            <span className="text-xs text-muted">
              {relative ? '超过时间窗的日志已隐藏' : `范围 ${windowLabel}`} · 可查看{' '}
              {earliestDay ?? '—'} ~ {today}
            </span>
            <span className="flex-1" />
            {view === 'action' && (
              <>
                <Select
                  value={outcome}
                  title="动作结果"
                  onChange={(e) => setOutcome(e.target.value)}
                >
                  <option value="">全部结果</option>
                  <option value="ok">成功</option>
                  <option value="skip">跳过</option>
                  <option value="fail">失败</option>
                  <option value="retry">待重试</option>
                  <option value="partial">部分成功</option>
                </Select>
                <Select
                  value={trigger}
                  title="触发方"
                  onChange={(e) => setTrigger(e.target.value)}
                >
                  <option value="">全部触发</option>
                  <option value="user">手动</option>
                  <option value="auto">自动</option>
                  <option value="schedule">定时</option>
                  <option value="startup">启动</option>
                </Select>
                <Input
                  placeholder="主体 / 账号"
                  className="w-28"
                  value={subject}
                  onChange={(e) => setSubject(e.target.value)}
                />
              </>
            )}
          </div>
        )}

        <div className="flex flex-wrap items-center gap-3 border-t border-line/60 px-2 py-1 text-xs">
          {view === 'claims' ? (
            <span className="text-muted">最近 {(claims.data ?? []).length} 条 · 按领取时间倒序</span>
          ) : (
            <>
              <span className="text-muted">
                当前结果 {view === 'stat' ? `${groups.data?.length ?? 0} 类` : `${stats.total} 条`}
              </span>
              {view !== 'alert' && (
                <button
                  className="cursor-pointer text-warn hover:underline"
                  onClick={openAlerts}
                  title="切到异常视图：只显示 warn / error"
                >
                  异常 {stats.abnormal} 次（错误 {errorCount} · 警告 {warnCount}）
                </button>
              )}
              {view === 'alert' && (
                <span className="text-warn">
                  异常 {stats.abnormal} 次（错误 {errorCount} · 警告 {warnCount}）· 只显示异常
                </span>
              )}
              <button
                className="cursor-pointer text-bad hover:underline"
                onClick={() => {
                  setView('action');
                  setOutcome('fail');
                }}
              >
                失败动作 {stats.failed}
              </button>
              <button
                className="cursor-pointer text-muted hover:text-fg"
                onClick={() => {
                  setView('action');
                  setOutcome('skip');
                }}
              >
                跳过 {stats.skipped}
              </button>
              {live.data?.length ? <span className="text-muted">实时 {live.data.length}</span> : null}
            </>
          )}
        </div>
      </div>

      {/* 新异常提示条：不遮挡列表，点一下切到异常视图；看过即推进水位 */}
      {hasNewAlert && (
        <div className="flex items-center gap-2 rounded-lg border border-warn/40 bg-warn/5 px-3 py-1.5 text-xs">
          <span className="font-medium text-warn">发现新的异常日志</span>
          <span className="text-muted">
            {windowLabel}内错误 {errorCount} 次、警告 {warnCount} 次
          </span>
          <span className="flex-1" />
          <Button size="sm" variant="ghost" onClick={openAlerts}>
            查看异常
          </Button>
          <button
            className="cursor-pointer text-muted hover:text-fg"
            onClick={() => markSeen(newestAlertAt(alertRows))}
            title="这批已经看过，不再提示"
          >
            知道了
          </button>
        </div>
      )}

      {/* 日志列表：页面上唯一会滚动的区域；「滚到底部」跟的是它 */}
      <div
        ref={listRef}
        className="min-h-0 flex-1 overflow-auto rounded-lg border border-line bg-card"
        onScroll={() => {
          const el = listRef.current;
          if (el) nearBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        <div className="p-2">
          {view === 'claims' ? (
            <ClaimsTable
              rows={claims.data ?? []}
              loading={claims.isLoading}
              error={claims.isError ? errText(claims.error) : null}
              onRetry={() => claims.refetch()}
            />
          ) : view === 'stat' ? (
            <StatTable
              stats={groups.data ?? []}
              loading={groups.isLoading}
              error={groups.isError ? errText(groups.error) : null}
              onRetry={() => groups.refetch()}
              onPick={(stat) => {
                // 点一类 → 回到明细并按这一类的模块/动作筛出来
                setView('all');
                setWindowKey('today');
                setModule(stat.module === 'action' ? '' : stat.module);
                setKeyword(stat.action ?? '');
              }}
            />
          ) : (
            <>
              {query.isLoading && <SkeletonBlock rows={8} />}
              {query.isError && (
                <StateBlock
                  kind="error"
                  title="读日志失败"
                  detail={errText(query.error)}
                  onRetry={() => query.refetch()}
                />
              )}
              {!query.isLoading && !query.isError && rows.length === 0 && (
                <p className="py-6 text-center text-sm text-muted">{emptyText}</p>
              )}
              {mainRows.map((line, i) => (
                <LogRow
                  key={`${line.ts}-${line.module}-${line.action ?? ''}-${i}`}
                  line={line}
                  onActivate={() => openContext(line)}
                  onContext={() => openContext(line)}
                  onDetail={() => setDetailLine(line)}
                />
              ))}
              {okRows.length > 0 && (
                <button
                  className="block w-full cursor-pointer border-b border-line px-2 py-1.5 text-left text-xs text-muted hover:bg-hover"
                  onClick={() => setShowOk((v) => !v)}
                >
                  {showOk ? '▾ 收起成功日志' : `▸ 成功日志 ${okRows.length} 条（默认折叠，点击展开）`}
                </button>
              )}
              {showOk &&
                okRows.map((line, i) => (
                  <LogRow
                    key={`${line.ts}-${line.module}-${line.action ?? ''}-ok-${i}`}
                    line={line}
                    onActivate={() => openContext(line)}
                    onContext={() => openContext(line)}
                    onDetail={() => setDetailLine(line)}
                  />
                ))}
              <div ref={tailRef} />
            </>
          )}
        </div>
      </div>

      <div className="flex items-center justify-between text-xs text-muted">
        <span>
          {view === 'claims'
            ? '领取记录来自本机台账：签到 / 活动 / 猫猫旅行，点一行跳到该账号'
            : '相同内容 60 秒内合并为一条，次数见 ×N · 点任意一行看上下文'}
          {view === 'action' && ' · 只显示真实动作与逐项记录，旧版只读命令已隐藏'}
          {view !== 'claims' && ' · 默认只看最近 10 分钟；更早的日志用上方时间窗回看'}
        </span>
        <button
          className="cursor-pointer hover:text-fg"
          onClick={() => {
            nearBottom.current = true;
            tailRef.current?.scrollIntoView({ block: 'end' });
          }}
        >
          回到底部
        </button>
      </div>

      <ContextDialog
        anchor={contextAnchor}
        onClose={() => setContextAnchor(null)}
        onOpenDetail={(line) => {
          setContextAnchor(null);
          setDetailLine(line);
        }}
      />
      <DetailDialog
        line={detailLine}
        onClose={() => setDetailLine(null)}
        onContext={(line) => {
          setDetailLine(null);
          openContext(line);
        }}
      />
    </div>
  );
}

/** 来源 → 中文标签：与台账写入侧的 source 取值一一对应。 */
const CLAIM_SOURCE: Record<string, string> = {
  checkin: '签到',
  travel: '猫猫旅行',
  activity: '活动',
};

/** 跨页跳转：账号中心挂载时消费 aim.focusAccount 并选中对应行。 */
function goAccount(iid: string) {
  sessionStorage.setItem('aim.focusAccount', iid);
  window.dispatchEvent(new CustomEvent('aim:navigate', { detail: 'account-center' }));
}

/**
 * 最近领取：从仪表盘迁来的「已经发生过的事」。
 * 放在日志页而不是设置页——它回答的是「发生过什么」，与执行动作同源。
 */
function ClaimsTable({
  rows,
  loading,
  error,
  onRetry,
}: {
  rows: RecentClaim[];
  loading: boolean;
  error: string | null;
  onRetry: () => void;
}) {
  // 骨架行必须在表格里：这个组件整体返回 <tr>，外面得给它一层 table
  if (loading) {
    return (
      <table className="w-full border-collapse">
        <tbody>
          <SkeletonRows rows={8} cols={5} />
        </tbody>
      </table>
    );
  }
  if (error) return <StateBlock kind="error" title="读领取记录失败" detail={error} onRetry={onRetry} />;
  if (rows.length === 0) {
    return (
      <p className="py-6 text-center text-sm text-muted">
        还没有领取记录。签到成功、领取活动或猫猫旅行领奖后，这里会出现明细。
      </p>
    );
  }
  return (
    <table className="w-full border-collapse text-xs">
      <thead>
        <tr className="text-left text-muted">
          <th className="w-40 py-1 font-medium">时间</th>
          <th className="w-44 py-1 font-medium">账号</th>
          <th className="w-24 py-1 font-medium">来源</th>
          <th className="w-24 py-1 text-right font-medium">积分</th>
          <th className="py-1 font-medium">说明</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((claim) => (
          <tr
            key={`${claim.iid}-${claim.claimed_at}-${claim.description}`}
            className="cursor-pointer border-t border-line/60 hover:bg-hover"
            onClick={() => goAccount(claim.iid)}
            title="跳到账号中心"
          >
            <td className="whitespace-nowrap py-1 tabular-nums text-muted">{fmtTime(claim.claimed_at)}</td>
            <td className="truncate py-1">{claim.name}</td>
            <td className="whitespace-nowrap py-1">
              <Tag tone={claim.source === 'travel' ? 'ok' : 'muted'}>
                {CLAIM_SOURCE[claim.source] ?? claim.source}
              </Tag>
            </td>
            <td className="whitespace-nowrap py-1 pr-4 text-right tabular-nums">
              {claim.amount === null ? '—' : `+${fmtAmount(claim.amount)}`}
            </td>
            <td className="max-w-[420px] truncate py-1 pl-1 text-fg2" title={claim.description}>
              {claim.description}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** 本地日期 → 当天 00:00:00 的 RFC3339（`YYYY-MM-DDT00:00:00` 按本地时区解析）。 */
function dayStartIso(day: string): string | null {
  return day ? new Date(`${day}T00:00:00`).toISOString() : null;
}

/** 本地日期 → 当天 23:59:59.999：日期筛选给的是整天，不能漏掉最后一秒的日志。 */
function dayEndIso(day: string): string | null {
  return day ? new Date(`${day}T23:59:59.999`).toISOString() : null;
}

/** 本地日期加减天数。先按本地构造再取年月日，避免 UTC 偏移把日期滑一天。 */
function shiftDay(day: string, offset: number): string {
  const date = new Date(`${day}T00:00:00`);
  date.setDate(date.getDate() + offset);
  return isoOf(date);
}

/** 本地日期（YYYY-MM-DD）：date 控件与后端按日口径都用它，不做时区换算。 */
function isoOf(date: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${p(date.getMonth() + 1)}-${p(date.getDate())}`;
}

function todayIso(date: Date): string {
  return isoOf(date);
}

/** RFC3339 → 本地日期：选择器的最小值来自日志首行，必须按本地日算。 */
function dayOf(ts: string): string {
  const date = new Date(ts);
  return Number.isNaN(date.getTime()) ? '' : isoOf(date);
}

/** RFC3339 转 YYYY-MM-DD HH:mm:ss.SSS：跨年也要能分清，毫秒保留先后顺序。 */
function shortTs(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const p = (n: number) => String(n).padStart(2, '0');
  return (
    `${d.getFullYear()}-` +
    p(d.getMonth() + 1) +
    '-' +
    p(d.getDate()) +
    ' ' +
    p(d.getHours()) +
    ':' +
    p(d.getMinutes()) +
    ':' +
    p(d.getSeconds()) +
    '.' +
    String(d.getMilliseconds()).padStart(3, '0')
  );
}

function effectiveOutcome(line: LogLine): string | null {
  if (line.outcome) return line.outcome;
  if (line.ok === true) return 'ok';
  if (line.ok === false) return 'fail';
  return null;
}

/** 实时行走与后端查询同一套过滤语义。 */
function matchFilter(line: LogLine, filter: LogFilter): boolean {
  if (filter.level && line.level !== filter.level) return false;
  if (filter.min_level && LEVEL_RANK[line.level] < LEVEL_RANK[filter.min_level]) return false;
  if (filter.module && !line.module.includes(filter.module)) return false;
  if (filter.action_only && line.module !== 'action') return false;
  if (filter.outcome && effectiveOutcome(line) !== filter.outcome) return false;
  if (filter.trigger && line.trigger !== filter.trigger) return false;
  if (filter.subject && !(line.subject ?? '').includes(filter.subject)) return false;
  if (filter.from || filter.to) {
    const ts = new Date(line.ts).getTime();
    if (Number.isNaN(ts)) return false;
    if (filter.from && ts < new Date(filter.from).getTime()) return false;
    if (filter.to && ts > new Date(filter.to).getTime()) return false;
  }
  if (filter.keyword) {
    // 与后端同口径：报错原文常常只在证据里，只搜 message 会「明明记了却搜不到」
    const haystack = [
      line.message,
      line.subject,
      line.step,
      line.evidence,
      line.action,
      line.batch,
    ]
      .filter(Boolean)
      .join(' ');
    if (!haystack.includes(filter.keyword)) return false;
  }
  return true;
}

async function copyText(text: string, message: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast(message);
  } catch {
    toast('复制失败，请手动选中复制');
  }
}

/**
 * 一行日志：抬头（时间/级别/主体/动作/结果/耗时）+ 正文 + 结构化字段。
 * 结构化字段单独一行而不是塞进正文：排障时唯一有用的部分不能被长句截断。
 * 点击整行打开上下文；异常行另有「详情」入口（弹窗按服务器日志的粒度展开）。
 */
function LogRow({
  line,
  forceExpanded = false,
  onActivate,
  onContext,
  onDetail,
  active = false,
}: {
  line: LogLine;
  forceExpanded?: boolean;
  onActivate?: () => void;
  onContext?: () => void;
  onDetail?: () => void;
  active?: boolean;
}) {
  const outcomeKey = effectiveOutcome(line);
  const outcome = outcomeKey ? OUTCOME[outcomeKey] : null;
  const failed = line.level === 'error' || line.level === 'warn';
  // 历史行里仍有整段 JSON：默认收起到三行，需要时再展开，避免一条日志淹一屏
  const long = line.message.length > 360;
  const [expanded, setExpanded] = useState(false);
  const open = forceExpanded || expanded;
  return (
    <div
      className={cn(
        'border-b border-line px-2 py-1 text-xs',
        onActivate && 'cursor-pointer hover:bg-hover/60',
        line.level === 'error' ? 'bg-bad/5' : line.level === 'warn' ? 'bg-warn/5' : '',
        active && 'ring-1 ring-accent/60',
      )}
      onClick={onActivate}
      title={onActivate ? '点击查看这一行的上下文' : undefined}
    >
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 font-mono">
        <span className="text-muted tabular-nums">{shortTs(line.ts)}</span>
        <span
          className={cn(
            'rounded px-1 text-[10px] font-semibold uppercase',
            line.level === 'error'
              ? 'bg-bad/15 text-bad'
              : line.level === 'warn'
                ? 'bg-warn/15 text-warn'
                : 'bg-hover text-muted',
          )}
        >
          {line.level}
        </span>
        {/* 合并行：同一句话重复 N 次只留这一条，次数必须显眼，否则会被当成「只发生了一次」 */}
        {line.repeat != null && line.repeat > 1 && (
          <span
            className="rounded bg-accent/15 px-1 text-[10px] font-semibold tabular-nums text-accent"
            title={
              line.last_ts
                ? `相同内容合并 ${line.repeat} 次，最近一次 ${shortTs(line.last_ts)}`
                : `相同内容合并 ${line.repeat} 次`
            }
          >
            ×{line.repeat}
          </span>
        )}
        {line.subject && (
          <span className="font-semibold text-fg" title={line.subject}>
            {line.subject}
          </span>
        )}
        <span className="text-fg2">{line.action ?? line.module}</span>
        {line.trigger && (
          <span className="text-muted">[{TRIGGER_LABEL[line.trigger] ?? line.trigger}]</span>
        )}
        {outcome && <span className={cn('font-medium', outcome.className)}>{outcome.text}</span>}
        {line.ms !== null && line.ms !== undefined && (
          <span className="text-muted tabular-nums">{fmtMs(line.ms)}</span>
        )}
        {line.batch && <span className="text-muted">批次={line.batch}</span>}
        <span className="ml-auto flex items-center gap-2">
          {failed && onDetail && (
            <button
              className="cursor-pointer font-sans text-[11px] font-medium text-bad hover:underline"
              onClick={(event) => {
                event.stopPropagation();
                onDetail();
              }}
              title="按服务器日志的粒度查看异常详情"
            >
              详情
            </button>
          )}
          {onContext && (
            <button
              className="cursor-pointer font-sans text-[11px] text-accent hover:underline"
              onClick={(event) => {
                event.stopPropagation();
                onContext();
              }}
              title="查看这一行前后的日志"
            >
              上下文
            </button>
          )}
        </span>
      </div>
      <div
        className={cn(
          'mt-0.5 whitespace-pre-wrap break-words pl-1',
          failed ? 'text-fg' : 'text-fg2',
          long && !open && 'line-clamp-3',
        )}
      >
        {line.message}
      </div>
      {long && !forceExpanded && (
        <button
          className="mt-0.5 cursor-pointer pl-1 text-[11px] text-accent hover:underline"
          onClick={(event) => {
            event.stopPropagation();
            setExpanded((v) => !v);
          }}
        >
          {open ? '收起' : `展开全文（${line.message.length} 字）`}
        </button>
      )}
      {(line.step || line.evidence || line.attempts) && (
        <div className="mt-0.5 flex flex-wrap gap-x-3 gap-y-0.5 pl-1 text-[11px]">
          {line.step && <span className="text-muted">环节={line.step}</span>}
          {line.evidence && (
            <span className="break-all text-muted" title={line.evidence}>
              证据={line.evidence}
            </span>
          )}
          {line.attempts && <span className="text-muted">重试={line.attempts}</span>}
        </div>
      )}
    </div>
  );
}

/**
 * 上下文弹窗：按文件物理行号取「前后各 N 行」，N 由用户现场决定。
 * 异常行在窗口里高亮并可继续打开详情——上下文回答因果，详情回答这一条到底报了什么。
 */
function ContextDialog({
  anchor,
  onClose,
  onOpenDetail,
}: {
  anchor: Anchor | null;
  onClose: () => void;
  onOpenDetail: (line: LogLine) => void;
}) {
  const [span, setSpan] = useState(20);
  const [onlyAlert, setOnlyAlert] = useState(false);
  const open = anchor !== null;
  const query = useQuery<LogContext>({
    queryKey: ['log-context', anchor?.anchor ?? null, anchor?.ts ?? null, span],
    queryFn: () =>
      queryCmd('log_context', {
        anchor: anchor?.anchor ?? null,
        ts: anchor?.ts ?? null,
        before: span,
        after: span,
      }),
    enabled: open,
    retry: false,
  });
  const lines = query.data?.lines ?? [];
  const visible = onlyAlert
    ? lines.filter((line) => line.level === 'warn' || line.level === 'error')
    : lines;

  return (
    <Dialog
      open={open}
      title="日志上下文"
      wide
      subtitle={
        anchor
          ? `${shortTs(anchor.ts)} 前后各 ${span} 行 · 共 ${lines.length} 行`
          : '这一行的前后日志'
      }
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            关闭
          </Button>
          <Button variant="ghost" onClick={() => void query.refetch()} disabled={!open}>
            重新读取
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted">
          <span>上下文行数</span>
          <Select
            value={String(span)}
            onChange={(e) => setSpan(Number(e.target.value) || 20)}
            title="前后各取多少行"
          >
            <option value="10">前后各 10 行</option>
            <option value="20">前后各 20 行</option>
            <option value="50">前后各 50 行</option>
            <option value="100">前后各 100 行</option>
            <option value="200">前后各 200 行</option>
          </Select>
          <label className="flex items-center gap-1">
            <Checkbox checked={onlyAlert} onChange={(e) => setOnlyAlert(e.target.checked)} />
            只看异常
          </label>
          <span className="flex-1" />
          {query.data && (
            <span className="font-mono">
              文件第 {query.data.from + 1}–{query.data.to} 行
            </span>
          )}
        </div>
        {query.isLoading && <SkeletonBlock rows={8} />}
        {query.isError && (
          <StateBlock
            kind="error"
            title="读取上下文失败"
            detail={errText(query.error)}
            onRetry={() => query.refetch()}
          />
        )}
        {!query.isLoading && !query.isError && lines.length === 0 && (
          <p className="py-6 text-center text-sm text-muted">
            日志文件里找不到这一行（可能已被清空或轮转）。重新写入后再试。
          </p>
        )}
        {visible.length > 0 && (
          <div className="max-h-[52vh] overflow-auto rounded-md border border-line bg-bg">
            {visible.map((line, index) => {
              const isAnchor = query.data?.anchor != null && line.seq === query.data.anchor;
              const failed = line.level === 'warn' || line.level === 'error';
              return (
                <div
                  key={`${line.seq ?? 'x'}-${line.ts}-${index}`}
                  className={cn(
                    'border-b border-line/60 px-2 py-1 font-mono text-[11px]',
                    failed ? 'bg-bad/5' : '',
                    isAnchor && 'bg-accent/10 ring-1 ring-accent/50',
                  )}
                >
                  <div className="flex flex-wrap items-baseline gap-x-2">
                    <span className="text-muted tabular-nums">{shortTs(line.ts)}</span>
                    <span
                      className={cn(
                        'rounded px-1 text-[10px] font-semibold uppercase',
                        line.level === 'error'
                          ? 'bg-bad/15 text-bad'
                          : line.level === 'warn'
                            ? 'bg-warn/15 text-warn'
                            : 'bg-hover text-muted',
                      )}
                    >
                      {line.level}
                    </span>
                    <span className="text-fg2">{line.module}</span>
                    {line.action && <span className="text-fg2">{line.action}</span>}
                    {line.subject && <span className="text-fg">{line.subject}</span>}
                    {isAnchor && <Tag tone="accent">锚点</Tag>}
                    {failed && (
                      <button
                        className="ml-auto cursor-pointer text-[11px] text-bad hover:underline"
                        onClick={() => onOpenDetail(line)}
                        title="打开异常详情"
                      >
                        详情
                      </button>
                    )}
                  </div>
                  <div className="mt-0.5 whitespace-pre-wrap break-words text-fg2">
                    {line.message}
                  </div>
                  {line.evidence && (
                    <div className="mt-0.5 break-all text-muted">证据={line.evidence}</div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </Dialog>
  );
}

/**
 * 异常详情弹窗：按后台服务器错误日志的粒度展示——
 * 概要字段、完整消息、证据原文、原始 JSON 四块，都可复制。
 * 这里是「这一条到底发生了什么」，要跳因果就转去上下文。
 */
function DetailDialog({
  line,
  onClose,
  onContext,
}: {
  line: LogLine | null;
  onClose: () => void;
  onContext: (line: LogLine) => void;
}) {
  if (!line) return null;
  const outcomeKey = effectiveOutcome(line);
  const outcome = outcomeKey ? OUTCOME[outcomeKey] : null;
  const raw = JSON.stringify(line, null, 2);
  const failed = line.level === 'warn' || line.level === 'error';
  const fields: { label: string; value: string }[] = [
    { label: '时间', value: shortTs(line.ts) },
    { label: '级别', value: line.level },
    { label: '模块', value: line.module },
    { label: '动作', value: line.action ?? '—' },
    { label: '触发', value: line.trigger ? (TRIGGER_LABEL[line.trigger] ?? line.trigger) : '—' },
    { label: '结果', value: outcome?.text ?? '—' },
    { label: '主体', value: line.subject ?? '—' },
    { label: '环节', value: line.step ?? '—' },
    { label: '耗时', value: line.ms === null || line.ms === undefined ? '—' : fmtMs(line.ms) },
    { label: '重试', value: line.attempts ?? '—' },
    { label: '批次', value: line.batch ?? '—' },
    { label: '合并', value: line.repeat && line.repeat > 1 ? `×${line.repeat}` : '—' },
  ];

  return (
    <Dialog
      open
      title={failed ? '异常详情' : '日志详情'}
      wide
      subtitle={`${shortTs(line.ts)} · ${line.level.toUpperCase()} · ${line.module}`}
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={() => void copyText(raw, '已复制完整日志（JSON）')}>
            复制详情
          </Button>
          <Button variant="ghost" onClick={() => onContext(line)}>
            查看上下文
          </Button>
          <Button variant="primary" onClick={onClose}>
            关闭
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <div
          className={cn(
            'rounded-md border border-line border-l-[3px] bg-bg px-3 py-2',
            line.level === 'error'
              ? 'border-l-bad'
              : line.level === 'warn'
                ? 'border-l-warn'
                : 'border-l-accent',
          )}
        >
          <div className="flex flex-wrap items-center gap-2">
            <span
              className={cn(
                'rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase',
                line.level === 'error'
                  ? 'bg-bad/15 text-bad'
                  : line.level === 'warn'
                    ? 'bg-warn/15 text-warn'
                    : 'bg-hover text-muted',
              )}
            >
              {line.level}
            </span>
            {outcome && <span className={cn('text-xs font-medium', outcome.className)}>{outcome.text}</span>}
            {line.action && <span className="font-mono text-xs text-fg2">{line.action}</span>}
            {line.subject && <span className="text-xs text-fg">{line.subject}</span>}
            <span className="flex-1" />
            {line.repeat && line.repeat > 1 && <Tag tone="accent">合并 ×{line.repeat}</Tag>}
          </div>
          <div className="mt-2 whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-fg">
            {line.message}
          </div>
        </div>

        <section>
          <h4 className="mb-1 text-xs font-semibold text-fg2">概要</h4>
          <div className="grid grid-cols-2 gap-x-3 gap-y-1 rounded-md border border-line bg-bg p-2.5 text-[11px] sm:grid-cols-3">
            {fields.map((field) => (
              <div key={field.label} className="flex min-w-0 gap-1">
                <span className="shrink-0 text-muted">{field.label}</span>
                <span className="min-w-0 break-all font-mono text-fg2">{field.value}</span>
              </div>
            ))}
          </div>
        </section>

        <section>
          <div className="mb-1 flex items-center gap-2">
            <h4 className="text-xs font-semibold text-fg2">证据</h4>
            <span className="text-[11px] text-muted">接口 / 状态码 / 错误原文</span>
            <span className="flex-1" />
            {line.evidence && (
              <button
                className="cursor-pointer text-[11px] text-accent hover:underline"
                onClick={() => void copyText(line.evidence ?? '', '已复制证据')}
              >
                复制证据
              </button>
            )}
          </div>
          {line.evidence ? (
            <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-all rounded-md border border-line bg-bg p-2.5 font-mono text-[11px] leading-relaxed text-fg2 select-all">
              {line.evidence}
            </pre>
          ) : (
            <p className="rounded-md border border-line bg-bg px-2.5 py-2 text-[11px] text-muted">
              这条日志没有记录证据字段。若是异常，说明失败分支漏了 `log::fail` 的证据参数。
            </p>
          )}
        </section>

        <section>
          <div className="mb-1 flex items-center gap-2">
            <h4 className="text-xs font-semibold text-fg2">原始 JSON</h4>
            <span className="text-[11px] text-muted">app.jsonl 的同一行</span>
          </div>
          <pre className="max-h-64 overflow-auto rounded-md border border-line bg-bg p-2.5 font-mono text-[11px] leading-relaxed text-muted select-all">
            {raw}
          </pre>
        </section>
      </div>
    </Dialog>
  );
}

/**
 * 聚合表：一类日志一行。
 * 次数按合并前的真实发生数算（`×N` 已展开），否则「同一条错 500 次」只会显示 1。
 */
function StatTable({
  stats,
  loading,
  error,
  onRetry,
  onPick,
}: {
  stats: LogStat[];
  loading: boolean;
  error: string | null;
  onRetry: () => void;
  onPick: (stat: LogStat) => void;
}) {
  // 同上：组件整体返回 <tr>，骨架自带一层 table 才画得出来
  if (loading) {
    return (
      <table className="w-full border-collapse">
        <tbody>
          <SkeletonRows rows={6} cols={6} />
        </tbody>
      </table>
    );
  }
  if (error) return <StateBlock kind="error" title="聚合失败" detail={error} onRetry={onRetry} />;
  if (stats.length === 0) {
    return <p className="py-6 text-center text-sm text-muted">没有可聚合的日志。</p>;
  }
  return (
    <table className="w-full border-collapse text-xs">
      <thead>
        <tr className="text-left text-muted">
          <th className="w-16 py-1 font-medium">次数</th>
          <th className="w-14 py-1 font-medium">级别</th>
          <th className="w-40 py-1 font-medium">模块 / 动作</th>
          <th className="w-16 py-1 font-medium">结果</th>
          <th className="w-36 py-1 font-medium">最近一次</th>
          <th className="py-1 font-medium">代表内容</th>
        </tr>
      </thead>
      <tbody>
        {stats.map((stat, index) => {
          const outcome = stat.outcome ? OUTCOME[stat.outcome] : null;
          return (
            <tr
              key={`${stat.module}-${stat.action ?? ''}-${stat.outcome ?? ''}-${stat.level}-${index}`}
              className="cursor-pointer border-t border-line hover:bg-hover"
              onClick={() => onPick(stat)}
              title="点击回到明细，并按这一类的模块与动作筛选"
            >
              <td className="whitespace-nowrap py-1 font-semibold tabular-nums">{stat.occurrences}</td>
              <td className="whitespace-nowrap py-1">
                <span
                  className={cn(
                    'rounded px-1 text-[10px] font-semibold uppercase',
                    stat.level === 'error'
                      ? 'bg-bad/15 text-bad'
                      : stat.level === 'warn'
                        ? 'bg-warn/15 text-warn'
                        : 'bg-hover text-muted',
                  )}
                >
                  {stat.level}
                </span>
              </td>
              <td className="truncate py-1" title={stat.action ?? stat.module}>
                {stat.action ?? stat.module}
                {stat.action && <span className="ml-1 text-muted">{stat.module}</span>}
              </td>
              <td className={cn('whitespace-nowrap py-1', outcome?.className ?? 'text-muted')}>
                {outcome?.text ?? '—'}
              </td>
              <td className="whitespace-nowrap py-1 tabular-nums text-muted">{shortTs(stat.last_ts)}</td>
              <td className="max-w-[420px] truncate py-1 text-fg2" title={stat.sample}>
                {stat.sample}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
