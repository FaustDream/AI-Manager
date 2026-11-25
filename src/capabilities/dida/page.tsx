// 滴答清单：口令配置 + 单个清单的任务台（新建 / 编辑 / 完成 / 删除 / 筛选）。
// 口令只在这里被用户输入一次，界面只显示「已配置 / 未配置」。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Plus, RefreshCw } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { Notice, Tag } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { Section } from '@/components/ui/card';
import { confirmAction } from '@/components/ui/confirm';
import { Field, Input, Select, SkeletonBlock, StateBlock } from '@/components/ui/form';
import { DataTable } from '@/components/ui/table';
import { toast } from '@/components/ui/toast';
import { errMsg, queryCmd, runCmd } from '@/lib/cmd';
import type { DidaProject, DidaState, DidaTask } from '@/lib/types';
import { didaColumns, didaTrail } from './columns';
import { TaskEditor } from './editor';

/** 收件箱优先：新建任务默认落到收件箱，用户再改清单的次数最少。 */
function preferredProject(projects: DidaProject[]): string {
  const inbox = projects.find(
    (project) => !project.closed && /收件箱|inbox/i.test(project.name),
  );
  return (inbox ?? projects[0])?.id ?? '';
}

export default function DidaPage() {
  const qc = useQueryClient();
  const stateQ = useQuery<DidaState>({
    queryKey: ['dida-state'],
    queryFn: () => queryCmd('dida_state'),
    retry: false,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });
  const state = stateQ.data ?? null;
  const projects = state?.projects ?? [];

  const [tokenDraft, setTokenDraft] = useState('');
  const [savingToken, setSavingToken] = useState(false);
  const [projectId, setProjectId] = useState('');
  const [keyword, setKeyword] = useState('');
  const [editorOpen, setEditorOpen] = useState(false);
  const [editing, setEditing] = useState<DidaTask | null>(null);

  // 清单变化（首屏、重填口令后）时把选中项收敛到一个仍然存在的清单
  useEffect(() => {
    if (!projects.length) {
      setProjectId('');
      return;
    }
    if (!projects.some((project) => project.id === projectId)) {
      setProjectId(preferredProject(projects));
    }
  }, [projects, projectId]);

  const tasksQ = useQuery<DidaTask[]>({
    queryKey: ['dida-tasks', projectId],
    queryFn: () => queryCmd('dida_tasks', { project_id: projectId }),
    enabled: Boolean(state?.configured) && projectId !== '',
    retry: false,
    refetchOnWindowFocus: false,
  });

  const rows = useMemo(() => {
    const all = tasksQ.data ?? [];
    const key = keyword.trim().toLowerCase();
    if (!key) return all;
    return all.filter(
      (task) =>
        task.title.toLowerCase().includes(key) ||
        (task.content ?? '').toLowerCase().includes(key),
    );
  }, [tasksQ.data, keyword]);

  const reloadTasks = () => {
    void qc.invalidateQueries({ queryKey: ['dida-tasks'] });
  };

  const saveToken = async () => {
    const token = tokenDraft.trim();
    if (!token) {
      toast('请填写 API 口令');
      return;
    }
    setSavingToken(true);
    const next = await runCmd('dida_token_set', { token });
    setSavingToken(false);
    if (!next) return;
    qc.setQueryData(['dida-state'], next);
    setTokenDraft('');
    toast('口令已保存，可以管理任务了');
  };

  const clearToken = async () => {
    const ok = await confirmAction({
      title: '清除滴答清单口令？',
      message: '清除后需要重新到网页版生成 API 口令并填写，才能继续管理任务。',
      danger: true,
      okText: '清除',
    });
    if (!ok) return;
    const next = await runCmd('dida_token_clear');
    if (!next) return;
    qc.setQueryData(['dida-state'], next);
    qc.removeQueries({ queryKey: ['dida-tasks'] });
    toast('已清除口令');
  };

  const completeTask = async (task: DidaTask) => {
    const done = await runCmd('dida_task_complete', {
      project_id: task.project_id || projectId,
      task_id: task.id,
    });
    if (done === undefined) return;
    reloadTasks();
    toast(`已完成「${task.title}」`);
  };

  const deleteTask = async (task: DidaTask) => {
    const ok = await confirmAction({
      title: `删除任务「${task.title}」？`,
      message: '滴答清单对应清单里的这条任务会一并删除，无法在这里撤销。',
      danger: true,
      okText: '删除',
    });
    if (!ok) return;
    const done = await runCmd('dida_task_delete', {
      project_id: task.project_id || projectId,
      task_id: task.id,
    });
    if (done === undefined) return;
    reloadTasks();
    toast('任务已删除');
  };

  if (stateQ.isLoading) {
    return (
      <div className="stagger flex flex-col gap-5">
        <Section title="滴答清单">
          <SkeletonBlock rows={5} />
        </Section>
      </div>
    );
  }

  if (stateQ.isError) {
    return (
      <div className="stagger flex flex-col gap-5">
        <Section title="滴答清单">
          <StateBlock
            kind="error"
            title="读取滴答清单状态失败"
            detail={errMsg(stateQ.error)}
            onRetry={() => stateQ.refetch()}
          />
        </Section>
      </div>
    );
  }

  if (!state?.configured) {
    return (
      <div className="stagger flex flex-col gap-5">
        <Section title="连接滴答清单">
          <StateBlock
            kind="empty"
            title="还没有配置 API 口令"
            detail="滴答清单网页版 → 头像 → 设置 → 账户与安全 → API 口令"
          />
          <div className="mt-3 flex flex-wrap items-end gap-2">
            <Field label="API 口令" className="min-w-[16rem] flex-1">
              <Input
                type="password"
                autoComplete="off"
                value={tokenDraft}
                onChange={(event) => setTokenDraft(event.target.value)}
                placeholder="粘贴网页版生成的 API 口令"
              />
            </Field>
            <Button variant="primary" onClick={() => void saveToken()} loading={savingToken}>
              保存
            </Button>
          </div>
        </Section>
      </div>
    );
  }

  return (
    <div className="stagger flex flex-col gap-5">
      <Section
        title="任务"
        note={
          <span className="flex items-center gap-2">
            <Tag tone="ok" dot>
              已配置
            </Tag>
            {projects.length > 0 && (
              <span className="text-xs text-muted">显示 {rows.length} 条</span>
            )}
          </span>
        }
        actions={
          <div className="flex items-center gap-1">
            <Button
              size="sm"
              variant="ghost"
              title="重新读取清单与任务"
              onClick={() => {
                void stateQ.refetch();
                reloadTasks();
              }}
            >
              <RefreshCw size={13} />
              刷新
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void clearToken()}>
              清除口令
            </Button>
          </div>
        }
      >
        {state.error && (
          <div className="mb-4">
            <Notice tone="bad">
              {state.error}
              <button
                type="button"
                className="ml-2 cursor-pointer underline underline-offset-2"
                onClick={() => void stateQ.refetch()}
              >
                重试
              </button>
            </Notice>
          </div>
        )}

        {projects.length === 0 ? (
          state.error ? null : (
            <StateBlock
              kind="empty"
              title="没有可写入的清单"
              detail="滴答清单里至少需要一个清单；新建后点上方「重试」重新读取。"
              onRetry={() => stateQ.refetch()}
            />
          )
        ) : (
          <div className="flex flex-col gap-3">
            <div className="flex flex-wrap items-end gap-2">
              <Field label="清单" htmlFor="dida-project" className="min-w-[10rem]">
                <Select
                  id="dida-project"
                  value={projectId}
                  onChange={(event) => setProjectId(event.target.value)}
                >
                  {projects.map((project) => (
                    <option key={project.id} value={project.id}>
                      {project.name}
                      {project.closed ? '（已关闭）' : ''}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label="搜索" htmlFor="dida-search" className="min-w-[10rem] flex-1">
                <Input
                  id="dida-search"
                  value={keyword}
                  onChange={(event) => setKeyword(event.target.value)}
                  placeholder="按标题或内容过滤"
                />
              </Field>
              <Button
                variant="primary"
                onClick={() => {
                  setEditing(null);
                  setEditorOpen(true);
                }}
              >
                <Plus size={14} />
                新建任务
              </Button>
            </div>

            {tasksQ.isError && (
              <Notice tone="bad">
                {errMsg(tasksQ.error)}
                <button
                  type="button"
                  className="ml-2 cursor-pointer underline underline-offset-2"
                  onClick={reloadTasks}
                >
                  重试
                </button>
              </Notice>
            )}

            <div className="overflow-x-auto rounded-lg border border-line bg-card">
              <DataTable
                columns={didaColumns()}
                rows={rows}
                rowKey={(task) => task.id}
                trail={didaTrail(
                  (task) => void completeTask(task),
                  (task) => {
                    setEditing(task);
                    setEditorOpen(true);
                  },
                  (task) => void deleteTask(task),
                )}
                loading={tasksQ.isLoading}
                skeletonRows={5}
                defaultSort={{ key: 'due', dir: 'asc' }}
                empty={
                  keyword.trim() ? (
                    <StateBlock kind="empty" title="没有匹配的任务" detail="换个关键词，或清空搜索框。" />
                  ) : (
                    <StateBlock
                      kind="empty"
                      title="这个清单还没有任务"
                      detail="点「新建任务」写入第一条。"
                    />
                  )
                }
              />
            </div>
          </div>
        )}
      </Section>

      <TaskEditor
        open={editorOpen}
        task={editing}
        projects={projects}
        defaultProjectId={projectId}
        onClose={() => setEditorOpen(false)}
        onSaved={() => {
          setEditorOpen(false);
          reloadTasks();
        }}
      />
    </div>
  );
}
