// 任务编辑器：新建与编辑共用一个抽屉，字段覆盖滴答开放平台可写全集。
//
// 子任务有两种，界面分开表达：
//   - 清单子项（items[]）：任务内部的勾选条目，这里可直接增删改；
//   - 真父子任务（parentId）：子任务本身是完整任务，在这里选父任务。
// 时间按任务时区解释，不再一律当成北京时间。
import { useEffect, useMemo, useState } from 'react';
import { GripVertical, Plus, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Drawer } from '@/components/ui/drawer';
import { Field, Input, Textarea } from '@/components/ui/form';
import { Checkbox } from '@/components/ui/form';
import { Hint, Picker, Switch } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd } from '@/lib/cmd';
import type { DidaItem, DidaProject, DidaTask, DidaTaskInput } from '@/lib/types';
import {
  DEFAULT_ZONE,
  PRIORITIES,
  REMINDERS,
  REPEATS,
  ZONES,
  fromDidaInZone,
  toDidaInZone,
} from './format';

/** 编辑中的子项：带一个稳定的本地 key，删中间一行时输入框不会串位。 */
interface DraftItem extends DidaItem {
  key: string;
}

let itemSeq = 0;
const nextKey = () => `draft-${++itemSeq}`;

export function TaskEditor({
  open,
  task,
  projects,
  tasks,
  defaultProjectId,
  defaultParentId,
  onClose,
  onSaved,
}: {
  open: boolean;
  /** null = 新建；否则回填该任务并走更新。 */
  task: DidaTask | null;
  projects: DidaProject[];
  /** 同清单的候选父任务（真父子关系要求父子同清单）。 */
  tasks: DidaTask[];
  defaultProjectId: string;
  /** 从某个父任务的「加子任务」进来时预填。 */
  defaultParentId?: string;
  onClose: () => void;
  onSaved: (task: DidaTask) => void;
}) {
  const [projectId, setProjectId] = useState(defaultProjectId);
  const [title, setTitle] = useState('');
  const [content, setContent] = useState('');
  const [desc, setDesc] = useState('');
  const [start, setStart] = useState('');
  const [due, setDue] = useState('');
  const [allDay, setAllDay] = useState(false);
  const [zone, setZone] = useState(DEFAULT_ZONE);
  const [priority, setPriority] = useState('0');
  const [reminder, setReminder] = useState('');
  const [repeat, setRepeat] = useState('');
  const [tags, setTags] = useState('');
  const [parentId, setParentId] = useState('');
  const [items, setItems] = useState<DraftItem[]>([]);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open) return;
    const taskZone = task?.time_zone || DEFAULT_ZONE;
    setProjectId(task?.project_id || defaultProjectId);
    setTitle(task?.title ?? '');
    setContent(task?.content ?? '');
    setDesc(task?.desc ?? '');
    setZone(taskZone);
    setAllDay(task?.is_all_day ?? false);
    setStart(fromDidaInZone(task?.start_date ?? null, taskZone));
    setDue(fromDidaInZone(task?.due_date ?? null, taskZone));
    setPriority(String(task?.priority ?? 0));
    setReminder(task?.reminders?.[0] ?? '');
    setRepeat(task?.repeat_flag ?? '');
    setTags((task?.tags ?? []).join(', '));
    setParentId(task?.parent_id || defaultParentId || '');
    setItems((task?.items ?? []).map((item) => ({ ...item, key: nextKey() })));
    setSaving(false);
  }, [open, task, defaultProjectId, defaultParentId]);

  // 候选父任务：同清单、非自身、非已完成。跨清单挂父会被平台拒绝，先在这里挡掉。
  const parentOptions = useMemo(
    () => [
      { value: '', label: '无（顶层任务）' },
      ...tasks
        .filter((t) => t.project_id === projectId && t.id !== task?.id && t.status !== 2)
        .map((t) => ({ value: t.id, label: t.title })),
    ],
    [tasks, projectId, task?.id],
  );

  const patchItem = (key: string, patch: Partial<DraftItem>) =>
    setItems((list) => list.map((item) => (item.key === key ? { ...item, ...patch } : item)));

  const save = async () => {
    if (!projectId) {
      toast('请选择清单');
      return;
    }
    if (!title.trim()) {
      toast('请填写任务标题');
      return;
    }
    if (start && due && start > due) {
      toast('开始时间不能晚于截止时间');
      return;
    }
    setSaving(true);
    const input: DidaTaskInput = {
      id: task?.id ?? '',
      title: title.trim(),
      project_id: projectId,
      content: content.trim() || null,
      desc: desc.trim() || null,
      start_date: toDidaInZone(start, allDay, zone),
      due_date: toDidaInZone(due, allDay, zone),
      is_all_day: allDay,
      time_zone: zone,
      priority: Number(priority),
      reminders: reminder ? [reminder] : [],
      repeat_flag: repeat || null,
      tags: tags
        .split(/[,，]/)
        .map((t) => t.trim())
        .filter(Boolean),
      items: items
        .filter((item) => item.title.trim())
        .map((item, index) => ({
          id: item.id,
          title: item.title.trim(),
          status: item.status,
          sort_order: index,
          start_date: item.start_date,
          is_all_day: item.is_all_day,
          time_zone: item.time_zone,
          completed_time: item.completed_time,
        })),
      // 明确给出（含空串）：空串 = 解除父子关系。新建时若无父则不发。
      parent_id: task || parentId ? parentId : null,
      sort_order: task?.sort_order ?? null,
    };
    const next = await runCmd('dida_task_save', { input });
    setSaving(false);
    if (!next) return;
    onSaved(next);
    toast(task ? '任务已更新' : `已写入「${next.title}」`);
  };

  const doneCount = items.filter((i) => i.status === 1).length;

  return (
    <Drawer
      open={open}
      width={620}
      title={task ? '编辑任务' : defaultParentId ? '新建子任务' : '新建任务'}
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
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="清单" htmlFor="dida-editor-project">
            <Picker
              id="dida-editor-project"
              label="清单"
              value={projectId}
              onValueChange={(next) => {
                setProjectId(next);
                // 换清单后原父任务不再同清单，清掉避免提交被平台拒绝
                setParentId('');
              }}
              options={projects.map((p) => ({
                value: p.id,
                label: p.closed ? `${p.name}（已关闭）` : p.name,
              }))}
            />
          </Field>
          <Field label="父任务" htmlFor="dida-editor-parent" hint="真父子关系要求父子同清单">
            <Picker
              id="dida-editor-parent"
              label="父任务"
              value={parentId}
              onValueChange={setParentId}
              options={parentOptions}
            />
          </Field>
        </div>

        <Field label="任务标题" htmlFor="dida-editor-title">
          <Input
            id="dida-editor-title"
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="要做什么"
          />
        </Field>

        <Field label="备注" htmlFor="dida-editor-content" hint="单行备注，显示在列表标题下方">
          <Input
            id="dida-editor-content"
            value={content}
            onChange={(event) => setContent(event.target.value)}
            placeholder="补充说明或链接"
          />
        </Field>

        <Field label="描述" htmlFor="dida-editor-desc" hint="多行正文，只在详情里展开">
          <Textarea
            id="dida-editor-desc"
            value={desc}
            onChange={(event) => setDesc(event.target.value)}
            placeholder="验收标准、步骤、上下文"
          />
        </Field>

        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="开始时间" htmlFor="dida-editor-start" hint="留空则只设截止">
            <Input
              id="dida-editor-start"
              type={allDay ? 'date' : 'datetime-local'}
              value={allDay ? start.slice(0, 10) : start}
              onChange={(event) =>
                setStart(allDay ? `${event.target.value}T00:00` : event.target.value)
              }
            />
          </Field>
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
        </div>

        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="时区" htmlFor="dida-editor-zone" hint="上面的时间按这个时区解释">
            <Picker
              id="dida-editor-zone"
              label="时区"
              value={zone}
              onValueChange={setZone}
              options={ZONES}
            />
          </Field>
          <Field label="优先级" htmlFor="dida-editor-priority">
            <Picker
              id="dida-editor-priority"
              label="优先级"
              value={priority}
              onValueChange={setPriority}
              options={PRIORITIES.map((p) => ({ value: String(p.value), label: p.label }))}
            />
          </Field>
        </div>

        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="提醒" htmlFor="dida-editor-reminder" hint="相对截止时间的提前量">
            <Picker
              id="dida-editor-reminder"
              label="提醒"
              value={reminder}
              onValueChange={setReminder}
              options={REMINDERS}
            />
          </Field>
          <Field label="重复" htmlFor="dida-editor-repeat">
            <Picker
              id="dida-editor-repeat"
              label="重复"
              value={repeat}
              onValueChange={setRepeat}
              options={REPEATS}
            />
          </Field>
        </div>

        <Field label="标签" htmlFor="dida-editor-tags" hint="逗号分隔，留空清空全部标签">
          <Input
            id="dida-editor-tags"
            value={tags}
            onChange={(event) => setTags(event.target.value)}
            placeholder="工作, 本周"
          />
        </Field>

        <div className="flex items-center justify-between rounded-md border border-line bg-bg px-2.5 py-2">
          <span className="text-sm text-fg2">全天任务（只记日期）</span>
          <Switch
            checked={allDay}
            onCheckedChange={setAllDay}
            label="全天任务"
            hint="开启后只保存日期，时刻归零"
          />
        </div>

        {/* 清单子项：与真父子任务分开，标题写清避免混淆 */}
        <div className="rounded-md border border-line">
          <div className="flex items-center justify-between border-b border-line bg-sunken px-2.5 py-1.5">
            <span className="text-xs font-semibold text-muted">
              清单子项
              {items.length > 0 && (
                <span className="ml-1.5 font-normal tabular-nums">
                  {doneCount}/{items.length}
                </span>
              )}
            </span>
            <Button
              size="sm"
              variant="ghost"
              onClick={() =>
                setItems((list) => [
                  ...list,
                  {
                    key: nextKey(),
                    id: '',
                    title: '',
                    status: 0,
                    sort_order: list.length,
                    start_date: null,
                    is_all_day: false,
                    time_zone: null,
                    completed_time: null,
                  },
                ])
              }
            >
              <Plus size={13} />
              加一项
            </Button>
          </div>
          {items.length === 0 ? (
            <p className="px-2.5 py-3 text-xs text-muted">
              还没有子项。子项是任务内部的勾选条目；要建独立的任务请选上面的「父任务」。
            </p>
          ) : (
            <ul className="flex flex-col">
              {items.map((item, index) => (
                <li
                  key={item.key}
                  className="flex items-center gap-2 border-b border-line px-2.5 py-1.5 last:border-b-0"
                >
                  <Hint content="顺序即保存后的排序" focusable={false}>
                    <GripVertical size={13} aria-hidden className="shrink-0 text-muted" />
                  </Hint>
                  <Checkbox
                    checked={item.status === 1}
                    aria-label={`标记「${item.title || `第 ${index + 1} 项`}」完成`}
                    onChange={(event) =>
                      patchItem(item.key, { status: event.target.checked ? 1 : 0 })
                    }
                  />
                  <Input
                    className="min-w-0 flex-1 border-0 bg-transparent px-0 py-0.5 hover:border-0"
                    value={item.title}
                    aria-label={`第 ${index + 1} 项子任务标题`}
                    placeholder="子项标题"
                    onChange={(event) => patchItem(item.key, { title: event.target.value })}
                  />
                  <Button
                    size="sm"
                    variant="ghost"
                    aria-label={`删除第 ${index + 1} 项`}
                    onClick={() => setItems((list) => list.filter((i) => i.key !== item.key))}
                  >
                    <Trash2 size={13} />
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>

        {task && <AssignBlock task={task} />}
      </div>
    </Drawer>
  );
}

/**
 * 负责人：指派走独立端点（不是任务字段），且需要共享清单。
 * 个人清单读不到成员，这里说清为什么不能指派，而不是摆一个点了报错的下拉。
 */
function AssignBlock({ task }: { task: DidaTask }) {
  const [username, setUsername] = useState(task.assignee ?? '');
  const [busy, setBusy] = useState(false);
  const [members, setMembers] = useState<{ username: string; name: string | null }[] | null>(null);

  useEffect(() => {
    setUsername(task.assignee ?? '');
    let alive = true;
    queryCmd('dida_project_members', { project_id: task.project_id })
      .then((list) => alive && setMembers(list))
      .catch(() => alive && setMembers([]));
    return () => {
      alive = false;
    };
  }, [task.id, task.project_id, task.assignee]);

  const assign = async (name: string) => {
    setBusy(true);
    const ok = name
      ? await runCmd('dida_task_assign', {
          project_id: task.project_id,
          task_id: task.id,
          username: name,
        })
      : await runCmd('dida_task_unassign', {
          project_id: task.project_id,
          task_id: task.id,
        });
    setBusy(false);
    if (ok === undefined) return;
    setUsername(name);
    toast(name ? `已指派给 ${name}` : '已取消指派');
  };

  const noMembers = members !== null && members.length === 0;

  return (
    <Field
      label="负责人"
      htmlFor="dida-editor-assignee"
      hint={
        noMembers
          ? '这个清单没有成员：指派需要共享清单（在滴答里把清单共享给他人）'
          : '指派立即生效，与上面的保存互不影响'
      }
    >
      {noMembers ? (
        <p className="text-xs text-muted">个人清单不支持指派。</p>
      ) : (
        <div className="flex items-center gap-2">
          <Picker
            id="dida-editor-assignee"
            label="负责人"
            value={username}
            disabled={busy || members === null}
            onValueChange={(next) => void assign(next)}
            options={[
              { value: '', label: '未指派' },
              ...(members ?? []).map((m) => ({
                value: m.username,
                label: m.name ? `${m.name}（${m.username}）` : m.username,
              })),
            ]}
          />
          {busy && <span className="text-xs text-muted">提交中…</span>}
        </div>
      )}
    </Field>
  );
}