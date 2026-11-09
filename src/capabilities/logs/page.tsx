// 日志：全部日志 / 执行动作双视图、增强筛选、实时跟随与导出。
// 执行动作不再单独成页——两者本来就是同一份 app.jsonl，只是看的角度不同。
// 按类型分窗：错误/警告进独立对话框（进页默认展开），成功日志在主列表默认折叠；
// 两条窗口都是日志页的一部分，内容不出页面。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { queryCmd, runCmd } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { Button, cn } from '@/components/ui/button';
import { confirmAction } from '@/components/ui/confirm';
import { Dialog } from '@/components/ui/dialog';
import { Input, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { Tag } from '@/components/ui';
import { fmtAmount, fmtMs, fmtTime } from '@/lib/format';
import { ALERT_SEEN_KEY, newestAlertAt, shouldAlert } from './alerts';
import type { Level, LogFilter, LogLine, LogSpan, LogStat, RecentClaim } from '@/lib/types';

type View = 'all' | 'action' | 'stat' | 'claims';

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
  partial: { text: '部分成功', className: 'text-warn' },
};

export default function LogsPage() {
  return <LogsPane />;
}

function LogsPane() {
  const qc = useQueryClient();
  const [view, setView] = useState<View>('all');
  const [minLevel, setMinLevel] = useState<Level | null>(null);
  const [module, setModule] = useState('');
  const [keyword, setKeyword] = useState('');
  const [outcome, setOutcome] = useState('');
  const [trigger, setTrigger] = useState('');
  const [subject, setSubject] = useState('');
  // 日期窗口：null = 跟随「今天」。存 null 而不是存死日期，跨零点后默认窗口自动跟上，
  // 「默认状态」也就在类型里写死了：不选日期 = 只看今天。
  const [fromDay, setFromDay] = useState<string | null>(null);
  const [toDay, setToDay] = useState<string | null>(null);
  const [limit, setLimit] = useState(300);
  const [exporting, setExporting] = useState(false);
  const [clearing, setClearing] = useState(false);
  // 异常窗口：不再「进页就弹」，只对还没提示过的错误弹一次（见下面的水位）。
  const [alertsOpen, setAlertsOpen] = useState(false);
  // 提示水位 = 上次弹窗时窗口里最新那条错误的时间（毫秒）。
  // null = 还没从设置里读回来：读到之前不弹，避免拿 0 当基准把旧错误全弹一遍。
  const [seenAt, setSeenAt] = useState<number | null>(null);
  const seenRef = useRef(0);
  // 成功日志默认折叠，点开才显示
  const [showOk, setShowOk] = useState(false);

  // 默认窗口 = 今天：早于今天的日志按「过期」处理，不进默认列表；改日期才回看。
  const today = todayIso();
  const dayFrom = fromDay ?? today;
  const dayTo = toDay ?? today;
  const defaultWindow = fromDay === null && toDay === null;

  // 可选范围 = 现存日志的首尾。日志按大小轮转（没有固定保留天数），
  // 写死「最近 N 天」会出现「选了却一条没有」的空范围，所以上界现算。
  const span = useQuery<LogSpan>({
    queryKey: ['log-span'],
    queryFn: () => queryCmd('log_span'),
    retry: false,
  });
  const earliestDay = span.data?.earliest ? dayOf(span.data.earliest) : undefined;

  const applyRange = (kind: 'today' | 'week' | 'all') => {
    if (kind === 'today') {
      setFromDay(null);
      setToDay(null);
      return;
    }
    if (kind === 'week') {
      setFromDay(shiftDay(today, -6));
      setToDay(today);
      return;
    }
    setFromDay(earliestDay ?? today);
    setToDay(today);
  };
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
    // 「只看异常」用最低级别表达，一次覆盖 warn + error
    min_level: minLevel,
    module: module || null,
    keyword: keyword || null,
    subject: subject || null,
    action_only: view === 'action' ? true : null,
    outcome: view === 'action' ? outcome || null : null,
    trigger: view === 'action' ? trigger || null : null,
    from: dayStartIso(dayFrom),
    to: dayEndIso(dayTo),
    hide_legacy: view === 'action' ? true : null,
    limit,
  };

  // 空态要说清是「今天没写」还是「这段日子没有」：默认窗口下直接说「没有日志」，
  // 用户会以为日志丢了，其实是过期日志被默认隐藏了。
  const emptyText =
    view === 'action'
      ? defaultWindow
        ? '今天还没有执行动作。每次点击、每次定时轮询都会留一条；要看更早的记录，改上方日期范围。'
        : '所选日期内没有执行动作。'
      : defaultWindow
        ? '今天还没有日志。要看更早的记录，改上方日期范围。'
        : '所选日期范围内没有日志。';

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
  const tailRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  // 贴底才自动跟随：用户向上翻历史时，新日志不许把视口拽走
  const nearBottom = useRef(true);

  const rows = useMemo(() => {
    const liveRows = (live.data ?? []).filter((line) => matchFilter(line, filter));
    const seen = new Set<string>();
    return [...(query.data ?? []), ...liveRows]
      .filter((line) => {
        // repeat 也要进 key：首行与合并行内容相同，只差次数
        const key = `${line.ts}|${line.module}|${line.action ?? ''}|${line.repeat ?? ''}|${line.message}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      })
      .sort((a, b) => a.ts.localeCompare(b.ts));
  }, [query.data, live.data, JSON.stringify(filter)]);

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

  // 类型分窗：异常行进对话框，成功行默认折叠；主列表只留其余行。
  // 用户在执行动作里显式筛「成功」时不折叠——那是用户点名要看的内容。
  const isOkRow = (line: LogLine) =>
    line.level !== 'warn' && line.level !== 'error' && effectiveOutcome(line) === 'ok';
  const collapseOk = !(view === 'action' && outcome === 'ok');
  const okRows = collapseOk ? rows.filter(isOkRow) : [];
  const mainRows = collapseOk ? rows.filter((line) => !isOkRow(line)) : rows;
  const alertRows = useMemo(
    () => rows.filter((line) => line.level === 'warn' || line.level === 'error'),
    [rows],
  );

  // 水位从 kv 读回：读失败按 0 处理（宁可多弹一次，也不能把错误吞掉）
  useEffect(() => {
    queryCmd('settings_get', { key: ALERT_SEEN_KEY })
      .then((value) => {
        const at = Number(value);
        seenRef.current = Number.isFinite(at) && at > 0 ? at : 0;
        setSeenAt(seenRef.current);
      })
      .catch(() => {
        seenRef.current = 0;
        setSeenAt(0);
      });
  }, []);

  const markSeen = useCallback((at: number) => {
    if (!Number.isFinite(at) || at <= seenRef.current) return;
    seenRef.current = at;
    setSeenAt(at);
    // 写失败只影响下次启动的起点，本会话仍以内存里的水位为准
    void queryCmd('settings_set', { key: ALERT_SEEN_KEY, value: at }).catch(() => {});
  }, []);

  // 关窗即视为「窗口里的内容已看过」：期间新到的错误一并推进水位，
  // 否则刚关窗就会被下一条新错误立刻再弹一次。
  const closeAlerts = () => {
    setAlertsOpen(false);
    markSeen(newestAlertAt(alertRows));
  };

  // 只对「比水位更新」的错误弹，且同一时刻只弹一次：
  // 打开后立刻推进水位，同一批日志不会再触发；重复渲染、重复取数也进不来。
  useEffect(() => {
    if (seenAt === null || alertsOpen || alertRows.length === 0) return;
    // 「最近领取」是用户主动切过去看的视图，弹窗不盖在它上面；
    // 切回日志视图时若仍有未提示的错误，这里会补弹。
    if (view === 'claims') return;
    if (!shouldAlert(alertRows, seenRef.current)) return;
    setAlertsOpen(true);
    markSeen(newestAlertAt(alertRows));
  }, [alertRows, seenAt, alertsOpen, view, markSeen]);

  useEffect(() => {
    if (nearBottom.current) {
      tailRef.current?.scrollIntoView({ block: 'end' });
    }
  }, [rows.length]);

  return (
    <div className="flex h-[calc(100vh-7.5rem)] flex-col gap-2">
      <div
        ref={listRef}
        className="min-h-0 flex-1 overflow-auto rounded-lg border border-line bg-card"
        onScroll={() => {
          const el = listRef.current;
          if (el) nearBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        <div className="sticky top-0 z-10 border-b border-line bg-bg">
          <div className="flex flex-wrap items-center gap-2 px-2 py-1.5">
            <div className="flex items-center gap-1 rounded-md border border-line p-0.5">
              <Button
                size="sm"
                variant={view === 'all' ? 'primary' : 'ghost'}
                onClick={() => {
                  // 日期窗口跨视图保留：它是「看哪段时间」，不该因为切视图被重置
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
                onClick={() => {
                  setView('claims');
                  // 异常窗口是日志视角的附属窗口，切到领取视图时不能盖在表格上
                  closeAlerts();
                }}
                title="本工具实际领取过的积分（签到 / 活动 / 猫猫旅行）"
              >
                最近领取
              </Button>
            </div>
            {view !== 'claims' && (
              <>
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
                <Select
                  value={String(limit)}
                  onChange={(e) => setLimit(Number(e.target.value) || 300)}
                >
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
                      // 日志被清空后可选范围也变了，日期选择器的上下界必须跟着重算
                      await qc.invalidateQueries({ queryKey: ['log-span'] });
                      // 清空后旧行即使还留在内存里也不该再弹：水位推到「现在」，
                      // 之后新写入的错误时间更晚，仍会正常触发
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
                  title="导出当前筛选结果（默认只含今天；改上方日期可导出更早的记录）"
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
              <label className="flex items-center gap-1 text-xs text-muted">
                日期
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
              <span className="flex items-center gap-0.5 rounded border border-line p-0.5">
                {(
                  [
                    ['today', '今天'],
                    ['week', '近 7 天'],
                    ['all', '全部'],
                  ] as const
                ).map(([kind, label]) => {
                  const active =
                    kind === 'today'
                      ? defaultWindow
                      : kind === 'week'
                        ? dayFrom === shiftDay(today, -6) && dayTo === today
                        : dayFrom === (earliestDay ?? today) && dayTo === today;
                  return (
                    <button
                      key={kind}
                      className={cn(
                        'cursor-pointer rounded px-1.5 py-0.5 text-xs',
                        active ? 'bg-hover text-fg' : 'text-muted hover:text-fg',
                      )}
                      onClick={() => applyRange(kind)}
                    >
                      {label}
                    </button>
                  );
                })}
              </span>
              <span className="text-xs text-muted">
                {defaultWindow ? '默认只看今天' : '按所选范围'} · 可查看{' '}
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
              <span className="text-muted">
                最近 {(claims.data ?? []).length} 条 · 按领取时间倒序
              </span>
            ) : (
              <>
                <span className="text-muted">
                  当前结果 {view === 'stat' ? `${groups.data?.length ?? 0} 类` : `${stats.total} 行`}
                </span>
                <button
                  className="cursor-pointer text-warn hover:underline"
                  onClick={() => setAlertsOpen(true)}
                  title="在异常窗口里查看全部错误与警告"
                >
                  异常 {stats.abnormal}
                </button>
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
                {live.data?.length ? (
                  <span className="text-muted">实时 {live.data.length}</span>
                ) : null}
              </>
            )}
          </div>
        </div>

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
                setModule(stat.module === 'action' ? '' : stat.module);
                setKeyword(stat.action ?? '');
              }}
            />
          ) : (
            <>
              {query.isLoading && <SkeletonRows rows={8} cols={1} />}
              {query.isError && (
                <StateBlock
                  kind="error"
                  title="读日志失败"
                  detail={errText(query.error)}
                  onRetry={() => query.refetch()}
                />
              )}
              {!query.isLoading && rows.length === 0 && (
                <p className="py-6 text-center text-sm text-muted">{emptyText}</p>
              )}
              {mainRows.map((line, i) => (
                <LogRow key={`${line.ts}-${line.module}-${line.action ?? ''}-${i}`} line={line} />
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
                  <LogRow key={`${line.ts}-${line.module}-${line.action ?? ''}-ok-${i}`} line={line} />
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
            : '相同内容 60 秒内合并为一条，次数见 ×N'}
          {view === 'action' && ' · 只显示真实动作与逐项记录，旧版只读命令已隐藏'}
          {view !== 'claims' && ' · 默认只看今天；更早（已过期）的日志用上方日期回看'}
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

      {/* 异常窗口：错误/警告独立输出，只有「比水位更新」的错误才会自动弹一次，
          条目完整展示不折叠。对话框只是日志页内的一扇窗——关掉后数据仍在主列表与导出里。 */}
      <Dialog
        open={alertsOpen}
        title={`异常窗口 · 错误与警告 ${alertRows.length} 条`}
        subtitle="只收错误与警告，正文默认完整展开；成功与普通日志在主列表查看"
        onClose={closeAlerts}
        wide
        footer={
          <>
            <Button
              variant="ghost"
              onClick={() => {
                setView('all');
                setMinLevel('warn');
                closeAlerts();
              }}
            >
              在列表中筛选
            </Button>
            <Button onClick={closeAlerts}>关闭</Button>
          </>
        }
      >
        {alertRows.length === 0 ? (
          <p className="py-6 text-center text-sm text-muted">当前筛选范围内没有错误或警告。</p>
        ) : (
          <div className="max-h-[60vh] overflow-y-auto rounded-md border border-line">
            {alertRows.map((line, i) => (
              <LogRow key={`${line.ts}-${line.module}-${line.action ?? ''}-alert-${i}`} line={line} forceExpanded />
            ))}
          </div>
        )}
      </Dialog>
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
  if (loading) return <SkeletonRows rows={8} cols={4} />;
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
            <td className="py-1 tabular-nums text-muted">{fmtTime(claim.claimed_at)}</td>
            <td className="truncate py-1">{claim.name}</td>
            <td className="py-1">
              <Tag tone={claim.source === 'travel' ? 'ok' : 'muted'}>
                {CLAIM_SOURCE[claim.source] ?? claim.source}
              </Tag>
            </td>
            <td className="py-1 pr-4 text-right tabular-nums">
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

function todayIso(): string {
  return isoOf(new Date());
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

/**
 * 一行日志：抬头（时间/级别/主体/动作/结果/耗时）+ 正文 + 结构化字段。
 * 结构化字段单独一行而不是塞进正文：排障时唯一有用的部分不能被长句截断。
 * `forceExpanded`：异常窗口里的行不折叠——错误原文就是要一眼看全。
 */
function LogRow({ line, forceExpanded = false }: { line: LogLine; forceExpanded?: boolean }) {
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
        line.level === 'error' ? 'bg-bad/5' : line.level === 'warn' ? 'bg-warn/5' : '',
      )}
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
          onClick={() => setExpanded((v) => !v)}
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
  if (loading) return <SkeletonRows rows={6} cols={4} />;
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
              <td className="py-1 font-semibold tabular-nums">{stat.occurrences}</td>
              <td className="py-1">
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
              <td className={cn('py-1', outcome?.className ?? 'text-muted')}>
                {outcome?.text ?? '—'}
              </td>
              <td className="py-1 tabular-nums text-muted">{shortTs(stat.last_ts)}</td>
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
