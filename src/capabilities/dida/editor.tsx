// 任务编辑器：新建与编辑共用一个抽屉，字段即滴答开放平台支持的全集。
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Drawer } from '@/components/ui/drawer';
import { Checkbox, Field, Input, Select, Textarea } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { runCmd } from '@/lib/cmd';
import type { DidaProject, DidaTask } from '@/lib/types';
import { PRIORITIES, fromDidaDue, toDidaDue } from './format';

export function TaskEditor({
  open,
  task,
  projects,
  defaultProjectId,
  onClose,
  onSaved,
}: {
  open: boolean;
  /** null = 新建；否则回填该任务并走更新。 */
  task: DidaTask | null;
  projects: DidaProject[];
  defaultProjectId: string;
  onClose: () => void;
  onSaved: (task: DidaTask) => void;
}) {
  const [projectId, setProjectId] = useState(defaultProjectId);
  const [title, setTitle] = useState('');
  const [content, setContent] = useState('');
  const [due, setDue] = useState('');
  const [allDay, setAllDay] = useState(false);
  const [priority, setPriority] = useState('0');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    setProjectId(task?.project_id || defaultProjectId);
    setTitle(task?.title ?? '');
    setContent(task?.content ?? '');
    setAllDay(task?.is_all_day ?? false);
    setDue(task ? fromDidaDue(task.due_date) : '');
    setPriority(String(task?.priority ?? 0));
    setSaving(false);
  }, [open, task, defaultProjectId]);

  const save = async () => {
    if (!projectId) {
      toast('请选择清单');
      return;
    }
    if (!title.trim()) {
      toast('请填写任务标题');
      return;
    }
    setSaving(true);
    const payload = {
      title: title.trim(),
      project_id: projectId,
      content: content.trim() || null,
      due_date: toDidaDue(due, allDay),
      priority: Number(priority),
      is_all_day: allDay,
    };
    const next = task
      ? await runCmd('dida_task_update', { ...payload, task_id: task.id })
      : await runCmd('dida_task_create', payload);
    setSaving(false);
    if (!next) return;
    onSaved(next);
    toast(task ? '任务已更新' : `已写入「${next.title}」`);
  };

  return (
    <Drawer
      open={open}
      title={task ? '编辑任务' : '新建任务'}
      subtitle={task ? task.title : '写入滴答清单指定清单'}
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            取消
          </Button>
          <Button variant="primary" loading={saving} onClick={() => void save()}>
            {task ? '保存' : '写入滴答清单'}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <Field label="清单" htmlFor="dida-editor-project">
          <Select
            id="dida-editor-project"
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

        <Field label="任务标题" htmlFor="dida-editor-title">
          <Input
            id="dida-editor-title"
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="要做什么"
          />
        </Field>

        <Field label="内容" htmlFor="dida-editor-content" hint="可留空；支持多行备注">
          <Textarea
            id="dida-editor-content"
            value={content}
            onChange={(event) => setContent(event.target.value)}
            placeholder="补充说明、链接或验收标准"
          />
        </Field>

        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="截止时间" htmlFor="dida-editor-due" hint="留空则不设置截止时间">
            <Input
              id="dida-editor-due"
              type={allDay ? 'date' : 'datetime-local'}
              value={allDay ? due.slice(0, 10) : due}
              onChange={(event) =>
                setDue(allDay ? `${event.target.value}T00:00` : event.target.value)
              }
            />
          </Field>
          <Field label="优先级" htmlFor="dida-editor-priority">
            <Select
              id="dida-editor-priority"
              value={priority}
              onChange={(event) => setPriority(event.target.value)}
            >
              {PRIORITIES.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </Select>
          </Field>
        </div>

        <label className="flex w-fit cursor-pointer items-center gap-2 text-sm text-fg2">
          <Checkbox checked={allDay} onChange={(event) => setAllDay(event.target.checked)} />
          全天任务（只记日期）
        </label>
      </div>
    </Drawer>
  );
}
