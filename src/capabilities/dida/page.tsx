// 滴答清单任务台：多清单聚合 / 单清单、父子树、清单子项、已完成归档、
// 批量完成与移动、删除后撤销。
//
// 三条口径：
//   1. 未完成走 dida_tasks_undone（跨清单一次取回）或 dida_tasks（单清单）；
//      已完成走 dida_tasks_completed，按时间区间取，不混进未完成表。
//   2. 删除是真删除（平台没有回收站接口），所以先确认、删完给撤销窗口——
//      撤销按原字段重建任务，并明确告知任务 id 会变。
//   3. 乐观更新只用于「勾选子项」这类本地可逆动作；跨设备写入一律等回执。
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Archive, ListTodo, Plus, RefreshCw, Trash2, FolderInput, CheckCheck } from 'lucide-react';
import {
  Button,
  Checkbox,
  Field,
  Hint,
  Input,
  Notice,
  Picker,
  Section,
  SkeletonBlock,
  StateBlock,
  Tag,
  confirmAction,
  toast,
} from '@/components/ui';
import { DataTable } from '@/components/ui/table';
import { queryCmd, runCmd } from '@/lib/cmd';
import type { DidaTask } from '@/lib/types';
import { TaskEditor } from './editor';
import { didaColumns, didaTrail, type DidaRow } from './columns';
import { flattenTree, isCompleted } from './format';

type Scope = 'all' | 'project';
type View = 'open' | 'done';

/** 归档区间：平台按完成时间取，给几个常用窗口而不是让用户填两个时间戳。 */
const RANGES = [
  { value: '7', label: '最近 7 天' },
  { value: '30', label: '最近 30 天' },
  { value: '90', label: '最近 90 天' },
];

const isoAt = (daysFromNow: number) => {
  const dt = new Date(Date.now() + daysFromNow * 86400000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${dt.getFullYear()}-${pad(dt.getMonth() + 1)}-${pad(dt.getDate())}T${pad(dt.getHours())}:${pad(dt.getMinutes())}:00+0800`;
};

/** 删除后重建用的字段快照。平台没有恢复接口，只能按原样再建一条。 */
interface DeletedSnapshot {
  task: DidaTask;
  childIds: string[];
}

export default function DidaPage() {
  const qc = useQueryClient();
  const [scope, setScope] = useState<Scope>('all');
  const [projectId, setProjectId] = useState('');
  const [view, setView] = useState<View>('open');
  const [range, setRange] = useState('30');
  const [search, setSearch] = useState('');
  const [priorityFilter, setPriorityFilter] = useState('all');
  const [tagFilter, setTagFilter] = useState('all');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [editing, setEditing] = useState<DidaTask | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [childParent, setChildParent] = useState<string | undefined>();
  const [moveTarget, setMoveTarget] = useState('');
  const [undo, setUndo] = useState<DeletedSnapshot | null>(null);

  const stateQ = useQuery({
    queryKey: ['dida-state'],
    queryFn: () => queryCmd('dida_state'),
    staleTime: Infinity,
    retry: false,
  });
  const configured = stateQ.data?.configured ?? false;
  const projects = useMemo(() => stateQ.data?.projects ?? [], [stateQ.data?.projects]);
  const projectNames = useMemo(
    () => new Map(projects.map((p) => [p.id, p.name])),
    [projects],
  );

  // 默认落在收件箱：单清单视图的初始选择
  useEffect(() => {
    if (projectId || projects.length === 0) return;
    const inbox = projects.find((p) => /收件箱|inbox/i.test(p.name) || p.id === 'inbox');
    setProjectId((inbox ?? projects[0]).id);
  }, [projects, projectId]);

  const openQ = useQuery({
    queryKey: ['dida-open', scope, projectId],
    queryFn: () =>
      scope === 'all'
        ? queryCmd('dida_tasks_undone')
        : queryCmd('dida_tasks', { project_id: projectId }),
    enabled: configured && view === 'open' && (scope === 'all' || !!projectId),
    refetchOnWindowFocus: false,
  });

  const doneQ = useQuery({
    queryKey: ['dida-done', scope, projectId, range],
    queryFn: () =>
      queryCmd('dida_tasks_completed', {
        project_ids: scope === 'all' ? projects.map((p) => p.id) : [projectId],
        from: isoAt(-Number(range)),
        to: isoAt(1),
      }),
    enabled:
      configured &&
      view === 'done' &&
      (scope === 'all' ? projects.length > 0 : !!projectId),
    refetchOnWindowFocus: false,
  });

  const active = view === 'open' ? openQ : doneQ;
  const raw = useMemo(() => active.data ?? [], [active.data]);

  const tags = useMemo(
    () => Array.from(new Set(raw.flatMap((t) => t.tags))).sort(),
    [raw],
  );

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return raw.filter((task) => {
      if (view === 'open' && isCompleted(task)) return false;
      if (priorityFilter !== 'all' && String(task.priority) !== priorityFilter) return false;
      if (tagFilter !== 'all' && !task.tags.includes(tagFilter)) return false;
      if (!q) return true;
      return [task.title, task.content ?? '', task.desc ?? '', ...task.tags, ...task.items.map((i) => i.title)]
        .join(' ')
        .toLowerCase()
        .includes(q);
    });
  }, [raw, search, priorityFilter, tagFilter, view]);

  const rows: DidaRow[] = useMemo(() => flattenTree(filtered), [filtered]);

  const invalidate = useCallback(() => {
    qc.invalidateQueries({ queryKey: ['dida-open'] });
    qc.invalidateQueries({ queryKey: ['dida-done'] });
  }, [qc]);

  const openEditor = (task: DidaTask | null, parentId?: string) => {
    setEditing(task);
    setChildParent(parentId);
    setEditorOpen(true);
  };

  const completeTask = async (task: DidaTask) => {
    const ok = await runCmd('dida_task_complete', {
      project_id: task.project_id,
      task_id: task.id,
    });
    if (ok === undefined) return;
    toast(`已完成「${task.title}」`);
    invalidate();
  };

  /** 勾选清单子项：本地先改（可逆），回执失败再回滚。 */
  const toggleItem = async (task: DidaTask, index: number, done: boolean) => {
    const key = view === 'open' ? ['dida-open', scope, projectId] : ['dida-done', scope, projectId, range];
    const before = qc.getQueryData<DidaTask[]>(key);
    const nextItems = task.items.map((item, i) =>
      i === index ? { ...item, status: done ? 1 : 0 } : item,
    );
    qc.setQueryData<DidaTask[]>(key, (list) =>
      list?.map((t) => (t.id === task.id ? { ...t, items: nextItems } : t)),
    );
    const saved = await runCmd('dida_task_save', {
      input: {
        id: task.id,
        title: task.title,
        project_id: task.project_id,
        content: task.content,
        desc: task.desc,
        start_date: task.start_date,
        due_date: task.due_date,
        is_all_day: task.is_all_day,
        time_zone: task.time_zone,
        priority: task.priority,
        reminders: task.reminders,
        repeat_flag: task.repeat_flag,
        tags: task.tags,
        items: nextItems,
        parent_id: task.parent_id,
        sort_order: task.sort_order,
      },
    });
    if (saved === undefined) {
      qc.setQueryData(key, before);
      return;
    }
    qc.setQueryData<DidaTask[]>(key, (list) =>
      list?.map((t) => (t.id === task.id ? saved : t)),
    );
  };

  const deleteTask = async (task: DidaTask) => {
    const children = raw.filter((t) => t.parent_id === task.id);
    const ok = await confirmAction({
      title: '删除任务',
      message:
        children.length > 0
          ? `「${task.title}」下还有 ${children.length} 个子任务。删除父任务不会自动删子任务，它们会变成顶层任务。确定删除？`
          : `确定删除「${task.title}」？滴答没有回收站接口，删除后可在提示条里撤销（按原字段重建）。`,
      danger: true,
      okText: '删除',
    });
    if (!ok) return;
    const result = await runCmd('dida_task_delete', {
      project_id: task.project_id,
      task_id: task.id,
    });
    if (result === undefined) return;
    setUndo({ task, childIds: children.map((c) => c.id) });
    setSelected((set) => {
      const next = new Set(set);
      next.delete(task.id);
      return next;
    });
    invalidate();
    toast(`已删除「${task.title}」`);
  };

  const restore = async () => {
    if (!undo) return;
    const { task } = undo;
    const saved = await runCmd('dida_task_save', {
      input: {
        id: '',
        title: task.title,
        project_id: task.project_id,
        content: task.content,
        desc: task.desc,
        start_date: task.start_date,
        due_date: task.due_date,
        is_all_day: task.is_all_day,
        time_zone: task.time_zone,
        priority: task.priority,
        reminders: task.reminders,
        repeat_flag: task.repeat_flag,
        tags: task.tags,
        items: task.items,
        parent_id: task.parent_id || null,
        sort_order: task.sort_order,
      },
    });
    if (!saved) return;
    setUndo(null);
    invalidate();
    toast(`已重建「${saved.title}」（任务 id 已变，原子任务的父子关系需在客户端复核）`);
  };

  const selectedPairs = (): [string, string][] =>
    rows
      .filter((r) => selected.has(r.task.id))
      .map((r) => [r.task.project_id, r.task.id]);

  const bulkComplete = async () => {
    const pairs = selectedPairs();
    if (pairs.length === 0) return;
    if (pairs.length > 50) {
      toast('单次最多完成 50 条，请分批操作');
      return;
    }
    const count = await runCmd('dida_tasks_complete_many', { pairs });
    if (count === undefined) return;
    setSelected(new Set());
    invalidate();
    toast(`已完成 ${count} 条任务`);
  };

  const bulkMove = async () => {
    const pairs = selectedPairs();
    if (pairs.length === 0 || !moveTarget) return;
    const count = await runCmd('dida_tasks_move', { pairs, to_project_id: moveTarget });
    if (count === undefined) return;
    setSelected(new Set());
    setMoveTarget('');
    invalidate();
    toast(`已移动 ${count} 条任务`);
  };

  const bulkDelete = async () => {
    const pairs = selectedPairs();
    if (pairs.length === 0) return;
    const ok = await confirmAction({
      title: `删除 ${pairs.length} 条任务`,
      message: '滴答没有回收站接口，批量删除不提供撤销。确定继续？',
      danger: true,
      okText: '全部删除',
    });
    if (!ok) return;
    let done = 0;
    for (const [pid, tid] of pairs) {
      const r = await runCmd('dida_task_delete', { project_id: pid, task_id: tid });
      if (r !== undefined) done += 1;
    }
    setSelected(new Set());
    invalidate();
    toast(`已删除 ${done} / ${pairs.length} 条`);
  };

  // ---- 未配置 / 加载中 / 出错 ----
  if (stateQ.isLoading) {
    return (
      <Section title="滴答清单">
        <SkeletonBlock rows={5} />
      </Section>
    );
  }
  if (stateQ.isError) {
    return (
      <Section title="滴答清单">
        <StateBlock
          kind="error"
          title="读不到滴答清单状态"
          detail="后端没有响应，先看「日志 → 运行日志」里的原因。"
          onRetry={() => void stateQ.refetch()}
        />
      </Section>
    );
  }
  if (!configured) {
    return <TokenGate onSaved={() => qc.invalidateQueries({ queryKey: ['dida-state'] })} />;
  }

  const allChecked = rows.length > 0 && rows.every((r) => selected.has(r.task.id));
  const openCount = raw.filter((t) => !isCompleted(t)).length;
  const overdueCount = raw.filter(
    (t) => !isCompleted(t) && t.due_date && new Date(t.due_date.replace(/\+\d{4}$/, '+08:00')).getTime() < Date.now(),
  ).length;

  return (
    <div className="flex flex-col gap-3">
      <Section
        title="任务"
        note={
          <span className="flex flex-wrap items-center gap-1.5">
            <Tag tone="ok" dot>
              已配置
            </Tag>
            <span className="text-xs text-muted tabular-nums">
              {view === 'open' ? `${openCount} 条未完成` : `${raw.length} 条已完成`}
              {view === 'open' && overdueCount > 0 && ` · ${overdueCount} 条逾期`}
            </span>
          </span>
        }
        actions={
          <div className="flex flex-wrap items-center gap-1.5">
            <Button
              size="sm"
              variant={view === 'open' ? 'soft' : 'ghost'}
              aria-pressed={view === 'open'}
              onClick={() => {
                setView('open');
                setSelected(new Set());
              }}
            >
              <ListTodo size={13} />
              未完成
            </Button>
            <Button
              size="sm"
              variant={view === 'done' ? 'soft' : 'ghost'}
              aria-pressed={view === 'done'}
              onClick={() => {
                setView('done');
                setSelected(new Set());
              }}
            >
              <Archive size={13} />
              已完成
            </Button>
            <Button size="sm" variant="ghost" onClick={() => invalidate()}>
              <RefreshCw size={13} />
              刷新
            </Button>
            {view === 'open' && (
              <Button size="sm" variant="primary" onClick={() => openEditor(null)}>
                <Plus size={13} />
                新建任务
              </Button>
            )}
          </div>
        }
        flush
      >
        {stateQ.data?.error && (
          <div className="px-3 pt-3">
            <Notice tone="bad">
              {stateQ.data.error}
              <Button
                size="sm"
                variant="ghost"
                className="ml-2"
                onClick={() => void stateQ.refetch()}
              >
                重试
              </Button>
            </Notice>
          </div>
        )}

        <div className="flex flex-wrap items-end gap-2 border-b border-line bg-sunken px-3 py-2">
          <Field label="范围" htmlFor="dida-scope">
            <Picker
              id="dida-scope"
              label="范围"
              className="w-28"
              value={scope}
              onValueChange={(next) => {
                setScope(next as Scope);
                setSelected(new Set());
              }}
              options={[
                { value: 'all', label: '全部清单' },
                { value: 'project', label: '单个清单' },
              ]}
            />
          </Field>
          {scope === 'project' && (
            <Field label="清单" htmlFor="dida-project">
              <Picker
                id="dida-project"
                label="清单"
                className="w-40"
                value={projectId}
                onValueChange={(next) => {
                  setProjectId(next);
                  setSelected(new Set());
                }}
                options={projects.map((p) => ({
                  value: p.id,
                  label: p.closed ? `${p.name}（已关闭）` : p.name,
                }))}
              />
            </Field>
          )}
          {view === 'done' && (
            <Field label="完成时间" htmlFor="dida-range">
              <Picker
                id="dida-range"
                label="完成时间区间"
                className="w-32"
                value={range}
                onValueChange={setRange}
                options={RANGES}
              />
            </Field>
          )}
          <Field label="搜索" htmlFor="dida-search">
            <Input
              id="dida-search"
              className="w-52"
              value={search}
              aria-label="搜索任务"
              placeholder="标题 / 备注 / 标签 / 子项"
              onChange={(event) => setSearch(event.target.value)}
            />
          </Field>
          <Field label="优先级" htmlFor="dida-priority-filter">
            <Picker
              id="dida-priority-filter"
              label="按优先级筛选"
              className="w-24"
              value={priorityFilter}
              onValueChange={setPriorityFilter}
              options={[
                { value: 'all', label: '全部' },
                { value: '5', label: '高' },
                { value: '3', label: '中' },
                { value: '1', label: '低' },
                { value: '0', label: '无' },
              ]}
            />
          </Field>
          {tags.length > 0 && (
            <Field label="标签" htmlFor="dida-tag-filter">
              <Picker
                id="dida-tag-filter"
                label="按标签筛选"
                className="w-28"
                value={tagFilter}
                onValueChange={setTagFilter}
                options={[
                  { value: 'all', label: '全部标签' },
                  ...tags.map((t) => ({ value: t, label: t })),
                ]}
              />
            </Field>
          )}
          {(search || priorityFilter !== 'all' || tagFilter !== 'all') && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setSearch('');
                setPriorityFilter('all');
                setTagFilter('all');
              }}
            >
              清除筛选
            </Button>
          )}
          <span className="flex-1" />
          <Hint content="当前显示 / 取回总数" focusable={false}>
            <span className="text-xs text-muted tabular-nums">
              {rows.length} / {raw.length}
            </span>
          </Hint>
        </div>

        {undo && (
          <div className="border-b border-line px-3 py-2">
            <Notice tone="warn">
              已删除「{undo.task.title}」。
              <Button size="sm" variant="soft" className="ml-2" onClick={() => void restore()}>
                撤销（按原字段重建）
              </Button>
              <Button size="sm" variant="ghost" className="ml-1" onClick={() => setUndo(null)}>
                忽略
              </Button>
            </Notice>
          </div>
        )}

        {selected.size > 0 && (
          <div className="flex flex-wrap items-center gap-2 border-b border-line bg-hover px-3 py-2">
            <span className="text-sm text-fg2 tabular-nums">已选 {selected.size} 条</span>
            {view === 'open' && (
              <>
                <Button size="sm" variant="soft" onClick={() => void bulkComplete()}>
                  <CheckCheck size={13} />
                  批量完成
                </Button>
                <Picker
                  label="移动到清单"
                  className="w-36"
                  value={moveTarget}
                  onValueChange={setMoveTarget}
                  placeholder="移动到…"
                  options={projects
                    .filter((p) => !p.closed)
                    .map((p) => ({ value: p.id, label: p.name }))}
                />
                {moveTarget && (
                  <Button size="sm" variant="soft" onClick={() => void bulkMove()}>
                    <FolderInput size={13} />
                    确认移动
                  </Button>
                )}
              </>
            )}
            <Button size="sm" variant="danger" onClick={() => void bulkDelete()}>
              <Trash2 size={13} />
              批量删除
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
              取消选择
            </Button>
          </div>
        )}

        {active.isError ? (
          <div className="p-3">
            <StateBlock
              kind="error"
              title={view === 'done' ? '读不到已完成任务' : '读不到任务'}
              detail={
                view === 'done'
                  ? '已完成任务按完成时间区间取；平台若拒绝该端点，原因见「日志 → 执行动作」。'
                  : '先确认口令仍有效，再重试。'
              }
              onRetry={() => void active.refetch()}
            />
          </div>
        ) : (
          <DataTable<DidaRow>
            columns={didaColumns({
              projectNames: scope === 'all' ? projectNames : undefined,
              onToggleItem: view === 'open' ? (t, i, d) => void toggleItem(t, i, d) : undefined,
            })}
            rows={rows}
            rowKey={(row) => row.task.id}
            loading={active.isLoading}
            skeletonRows={6}
            stickyHead
            defaultSort={view === 'open' ? { key: 'due', dir: 'asc' } : undefined}
            lead={{
              width: 32,
              cell: (row) => (
                <Checkbox
                  checked={selected.has(row.task.id)}
                  aria-label={`选择「${row.task.title}」`}
                  onChange={(event) =>
                    setSelected((set) => {
                      const next = new Set(set);
                      if (event.target.checked) next.add(row.task.id);
                      else next.delete(row.task.id);
                      return next;
                    })
                  }
                />
              ),
            }}
            trail={didaTrail(
              (task) => void completeTask(task),
              (task) => openEditor(task),
              (task) => void deleteTask(task),
              view === 'open' ? (task) => openEditor(null, task.id) : undefined,
            )}
            empty={
              <StateBlock
                kind="empty"
                title={
                  raw.length === 0
                    ? view === 'done'
                      ? '这个区间里没有已完成任务'
                      : '没有未完成任务'
                    : '没有匹配的任务'
                }
                detail={
                  raw.length === 0
                    ? view === 'done'
                      ? '换一个更长的时间区间，或先在滴答里完成几条任务。'
                      : scope === 'all'
                        ? '跨清单聚合只覆盖平台提供的最近窗口；更早的任务请在滴答客户端里看。'
                        : '点「新建任务」写入第一条。'
                    : '当前筛选把任务挡住了，不是任务丢了。'
                }
                action={
                  raw.length > 0 ? (
                    <Button
                      size="sm"
                      variant="soft"
                      onClick={() => {
                        setSearch('');
                        setPriorityFilter('all');
                        setTagFilter('all');
                      }}
                    >
                      清除筛选
                    </Button>
                  ) : undefined
                }
              />
            }
          />
        )}

        {rows.length > 0 && (
          <div className="flex items-center gap-2 border-t border-line px-3 py-1.5">
            <Checkbox
              checked={allChecked}
              aria-label="全选当前列表"
              onChange={(event) =>
                setSelected(event.target.checked ? new Set(rows.map((r) => r.task.id)) : new Set())
              }
            />
            <span className="text-xs text-muted">全选当前列表</span>
            <span className="flex-1" />
            <span className="text-xs text-muted">
              子项可直接勾选；父子任务用操作列的「+」新建
            </span>
          </div>
        )}
      </Section>

      <TaskEditor
        open={editorOpen}
        task={editing}
        projects={projects}
        tasks={raw}
        defaultProjectId={editing?.project_id || projectId || projects[0]?.id || ''}
        defaultParentId={childParent}
        onClose={() => {
          setEditorOpen(false);
          setChildParent(undefined);
        }}
        onSaved={() => {
          setEditorOpen(false);
          setChildParent(undefined);
          invalidate();
        }}
      />
    </div>
  );
}

/** 未配置：填 API 口令。校验通过才落库，无效口令不占「已配置」位。 */
function TokenGate({ onSaved }: { onSaved: () => void }) {
  const [token, setToken] = useState('');
  const [saving, setSaving] = useState(false);
  const save = async () => {
    if (!token.trim()) {
      toast('请填写 API 口令');
      return;
    }
    setSaving(true);
    const next = await runCmd('dida_token_set', { token: token.trim() });
    setSaving(false);
    if (!next) return;
    setToken('');
    onSaved();
    toast('已连接滴答清单');
  };
  return (
    <Section title="连接滴答清单">
      <div className="flex flex-col gap-3">
        <StateBlock
          kind="empty"
          title="还没有连接滴答清单"
          detail="在滴答清单网页版 → 头像 → 设置 → 账户与安全 → API 口令 生成一串口令，粘贴到下面。口令只经本机加密保存，不进日志。"
        />
        <Field label="API 口令" htmlFor="dida-token">
          <Input
            id="dida-token"
            type="password"
            value={token}
            autoComplete="off"
            placeholder="粘贴滴答清单 API 口令"
            onChange={(event) => setToken(event.target.value)}
          />
        </Field>
        <div>
          <Button variant="primary" loading={saving} onClick={() => void save()}>
            保存并连接
          </Button>
        </div>
      </div>
    </Section>
  );
}