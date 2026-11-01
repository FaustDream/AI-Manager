// 设置中心：通用（主题/日志级别/开机自启）、日志（查询/导出）、任务（列表/取消/清理）、
// 维护（空间/回收站/打开目录/v1 归档）。事件桥已在 providers.tsx 接 task:state / log:line。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { call, errText } from '@/lib/ipc';
import { fmtSize } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Checkbox, Input, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { Notice, Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { saveTheme, type ThemeChoice } from '@/lib/theme';
import type { CountPair, LogFilter, LogLine, SpaceEntry, SpaceReport, TaskView } from '@/lib/types';

const TABS = [
  { id: 'general', label: '通用' },
  { id: 'logs', label: '日志' },
  { id: 'tasks', label: '任务' },
  { id: 'maintenance', label: '维护' },
] as const;

type Tab = (typeof TABS)[number]['id'];

export default function SettingsPage() {
  const [tab, setTab] = useState<Tab>('general');
  return (
    <div className="flex flex-col gap-3">
      <nav className="flex gap-1">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={`rounded-md px-2.5 py-1 text-sm ${tab === t.id ? 'bg-hover font-semibold text-fg' : 'text-muted hover:bg-hover hover:text-fg'}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>
      {tab === 'general' && <GeneralPane />}
      {tab === 'logs' && <LogsPane />}
      {tab === 'tasks' && <TasksPane />}
      {tab === 'maintenance' && <MaintenancePane />}
    </div>
  );
}

function GeneralPane() {
  const [theme, setTheme] = useState<ThemeChoice>('');
  const [autostart, setAutostart] = useState<boolean | null>(null);

  useEffect(() => {
    call<string | null>('settings_get', { key: 'ui.theme' })
      .then((v) => setTheme((v as ThemeChoice) || ''))
      .catch(() => {});
    call<boolean>('autostart_get')
      .then(setAutostart)
      .catch(() => setAutostart(false));
  }, []);

  return (
    <div className="flex max-w-[560px] flex-col gap-4 rounded-lg border border-line bg-card p-4">
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
        </Select>
      </label>
      <label className="flex items-center gap-4 text-sm">
        <span className="w-24 text-muted">开机自启</span>
        <Checkbox
          checked={autostart ?? false}
          disabled={autostart === null}
          onChange={async (e) => {
            const enabled = e.target.checked;
            setAutostart(enabled);
            try {
              await call('autostart_set', { enabled });
              toast(enabled ? '已开机自启（写 HKCU Run）' : '已关闭开机自启');
            } catch (err) {
              setAutostart(!enabled);
              toast(errText(err));
            }
          }}
        />
      </label>
      <p className="text-xs text-muted">
        日志级别在「日志」页调整；数据目录在「维护」页打开。
      </p>
    </div>
  );
}

function LogsPane() {
  const [filter, setFilter] = useState<LogFilter>({ level: null, module: null, keyword: null, limit: 300 });
  const [exporting, setExporting] = useState(false);
  const query = useQuery<LogLine[]>({
    queryKey: ['log', JSON.stringify(filter)],
    queryFn: () => call<LogLine[]>('log_query', { filter }),
  });
  const live = useQuery<LogLine[]>({
    queryKey: ['log-live'],
    queryFn: () => [],
    staleTime: Infinity,
  });
  const tailRef = useRef<HTMLDivElement | null>(null);
  const rows = [...(query.data ?? []), ...(live.data ?? [])];

  useEffect(() => {
    tailRef.current?.scrollIntoView();
  }, [rows.length]);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <Select
          value={filter.level ?? ''}
          onChange={(e) => setFilter((f) => ({ ...f, level: e.target.value || null }))}
        >
          <option value="">全部级别</option>
          <option value="info">info</option>
          <option value="warn">warn</option>
          <option value="error">error</option>
        </Select>
        <Input
          placeholder="模块"
          value={filter.module ?? ''}
          onChange={(e) => setFilter((f) => ({ ...f, module: e.target.value || null }))}
        />
        <Input
          placeholder="关键词"
          value={filter.keyword ?? ''}
          onChange={(e) => setFilter((f) => ({ ...f, keyword: e.target.value || null }))}
        />
        <Select
          value={filter.limit ? String(filter.limit) : '300'}
          onChange={(e) => setFilter((f) => ({ ...f, limit: Number(e.target.value) || null }))}
        >
          <option value="100">最近 100 行</option>
          <option value="300">最近 300 行</option>
          <option value="1000">最近 1000 行</option>
        </Select>
        <span className="flex-1" />
        <LogLevelPicker />
        <Button
          size="sm"
          loading={exporting}
          onClick={async () => {
            setExporting(true);
            try {
              toast(await call<string>('log_export', { path: null, filter }));
            } catch (e) {
              toast(errText(e));
            } finally {
              setExporting(false);
            }
          }}
        >
          导出
        </Button>
      </div>
      <div className="h-[60vh] overflow-auto rounded-lg border border-line bg-card p-2">
        {query.isLoading && <SkeletonRows rows={8} cols={1} />}
        {query.isError && (
          <StateBlock kind="error" title="读日志失败" detail={errText(query.error)} onRetry={() => query.refetch()} />
        )}
        {rows.map((line, i) => (
          <div
            key={i}
            className={`whitespace-pre-wrap break-words border-b border-line px-2 py-0.5 font-mono text-xs ${
              line.level === 'error' ? 'text-bad' : line.level === 'warn' ? 'text-warn' : ''
            }`}
          >
            {line.ts} [{line.level}] {line.module} · {line.message}
            {line.task ? ` · ${line.task}` : ''}
          </div>
        ))}
        <div ref={tailRef} />
      </div>
    </div>
  );
}

function LogLevelPicker() {
  const qc = useQueryClient();
  const [level, setLevel] = useState<string | null>(null);
  useEffect(() => {
    call<string | null>('settings_get', { key: 'log.level' })
      .then((v) => setLevel((v as string) || 'info'))
      .catch(() => {});
  }, []);
  return (
    <Select
      value={level ?? 'info'}
      title="运行日志级别"
      onChange={async (e) => {
        const value = e.target.value;
        setLevel(value);
        try {
          await call('log_level', { level: value });
          await call('settings_set', { key: 'log.level', value });
          toast(`日志级别 → ${value}`);
          qc.invalidateQueries({ queryKey: ['log'] });
        } catch (err) {
          toast(errText(err));
        }
      }}
    >
      <option value="debug">debug</option>
      <option value="info">info</option>
      <option value="warn">warn</option>
      <option value="error">error</option>
    </Select>
  );
}

function TasksPane() {
  const qc = useQueryClient();
  const [cleaning, setCleaning] = useState(false);
  const tasks = useQuery<TaskView[]>({
    queryKey: ['tasks'],
    queryFn: () => call<TaskView[]>('task_list'),
    refetchInterval: 2000,
  });
  const STATE_STYLE: Record<string, string> = {
    queued: 'muted', running: 'warn', cancelling: 'warn', done: 'ok', partial: 'warn', failed: 'bad', cancelled: 'muted',
  };
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          loading={cleaning}
          onClick={async () => {
            setCleaning(true);
            try {
              const n = await call<number>('task_clear_finished');
              toast(`清理了 ${n} 条已结束的任务`);
              qc.invalidateQueries({ queryKey: ['tasks'] });
            } catch (e) {
              toast(errText(e));
            } finally {
              setCleaning(false);
            }
          }}
        >
          清理已结束
        </Button>
      </div>
      <div className="rounded-lg border border-line bg-card p-2">
        {(tasks.data ?? []).length === 0 && <StateBlock kind="empty" title="没有后台任务" />}
        {(tasks.data ?? []).map((task) => (
          <div key={task.id} className="flex items-center gap-2 border-b border-line px-1 py-1.5 text-sm last:border-b-0">
            <Tag tone={(STATE_STYLE[task.state] as 'ok' | 'warn' | 'bad' | 'muted') ?? 'muted'}>
              {task.state}
            </Tag>
            <span className="min-w-0 flex-1 truncate" title={task.message}>
              {task.name}
              {task.message && <span className="text-muted"> · {task.message}</span>}
            </span>
            {task.progress !== null && (
              <span className="inline-block h-1.5 w-24 overflow-hidden rounded-full bg-hover">
                <span className="block h-full bg-accent" style={{ width: `${Math.round(task.progress * 100)}%` }} />
              </span>
            )}
            {(task.state === 'queued' || task.state === 'running' || task.state === 'cancelling') && (
              <Button
                size="sm"
                onClick={async () => {
                  await call('task_cancel', { taskId: task.id }).catch(() => {});
                  qc.invalidateQueries({ queryKey: ['tasks'] });
                }}
              >
                取消
              </Button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

function MaintenancePane() {
  const qc = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const runBusy = (key: string, work: () => Promise<void>) => {
    if (busy) return;
    setBusy(key);
    void work().finally(() => setBusy(null));
  };
  const space = useQuery<SpaceReport>({
    queryKey: ['space'],
    queryFn: () => call<SpaceReport>('maintenance_space'),
  });
  const trash = useQuery<SpaceEntry[]>({
    queryKey: ['maint-trash'],
    queryFn: () => call<SpaceEntry[]>('maintenance_trash'),
  });
  const [verify, setVerify] = useState<string | null>(null);

  return (
    <div className="flex max-w-[720px] flex-col gap-3">
      <div className="rounded-lg border border-line bg-card p-4">
        <div className="mb-2 flex items-center gap-2">
          <h2 className="text-sm font-semibold">数据目录占用</h2>
          <span className="flex-1" />
          <Button
            size="sm"
            onClick={async () => {
              try {
                await call('maintenance_open_data');
              } catch (e) {
                toast(errText(e));
              }
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
        <h2 className="mb-2 text-sm font-semibold">回收站（会话删除与平台技能移除）</h2>
        {(trash.data ?? []).length === 0 && <p className="text-sm text-muted">空的。</p>}
        {(trash.data ?? []).map((batch) => (
          <div key={batch.path} className="flex items-center gap-2 border-b border-line px-1 py-1 text-sm last:border-b-0">
            <span className="flex-1 truncate font-mono text-xs" title={batch.path}>{batch.name}</span>
            <span className="tabular-nums text-muted">{fmtSize(batch.size)}</span>
          </div>
        ))}
        {(trash.data ?? []).length > 0 && (
          <div className="mt-2 flex justify-end gap-2">
            <Button size="sm" onClick={() => qc.invalidateQueries({ queryKey: ['maint-trash'] })}>刷新</Button>
            <Button
              size="sm"
              variant="danger"
              loading={busy === 'purge'}
              disabled={!!busy}
              onClick={() =>
                runBusy('purge', async () => {
                  if (!window.confirm('清空整个回收站？不可恢复。')) return;
                  try {
                    const bytes = await call<number>('maintenance_purge_trash', { tag: null });
                    toast(`已释放 ${fmtSize(bytes)}`);
                    qc.invalidateQueries({ queryKey: ['maint-trash'] });
                    qc.invalidateQueries({ queryKey: ['space'] });
                  } catch (e) {
                    toast(errText(e));
                  }
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
                try {
                  const r = await call<{ accounts: CountPair; sessions: CountPair; ledger: CountPair; ok: boolean }>('migrate_verify');
                  const line = (name: string, pair: CountPair) =>
                    `${name} ${pair.v1}/${pair.v2}`;
                  setVerify(
                    `账号 ${line('', r.accounts)} · 会话 ${r.sessions.v1}/${r.sessions.v2} · 积分台账 ${r.ledger.v1}/${r.ledger.v2}\n` +
                      (r.ok ? '计数一致，可以归档。' : '计数不一致，先处理差异再归档。'),
                  );
                } catch (e) {
                  toast(errText(e));
                }
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
                try {
                  const r = await call<{ archive_dir: string }>('migrate_archive');
                  toast(`v1 文件已归档到 ${r.archive_dir}`);
                } catch (e) {
                  toast(errText(e));
                }
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
