// 会话的三个编辑弹窗：本地改名（锁定）、标签编辑、未命名会话批量治理。
// 都只写本地标记（marks / session_tag），客户端与扫描无感。
import { useMemo, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, Input } from '@/components/ui/form';
import { runCmd } from '@/lib/cmd';
import { toast } from '@/components/ui/toast';

/** 本地改名。空标题 = 恢复客户端原标题（按钮文案随之变化）。 */
export function RenameDialog({
  keyId,
  current,
  renamed,
  onClose,
  onDone,
}: {
  keyId: string;
  current: string;
  renamed: boolean;
  onClose: () => void;
  onDone: () => void;
}) {
  const [draft, setDraft] = useState(renamed ? current : '');
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    setBusy(true);
    const title = draft.trim();
    // 手动改名即锁定：扫描永不覆盖；清空 = 恢复原标题
    const ok = (await runCmd('session_rename', { key: keyId, title: title || null })) !== undefined;
    setBusy(false);
    if (!ok) return;
    toast(title ? `已重命名（本地命名不会被扫描覆盖）` : '已恢复客户端原标题');
    onDone();
    onClose();
  };

  return (
    <Dialog
      open
      title="重命名会话"
      subtitle="只改本工具里的显示名，客户端里的标题不动；改名后扫描不会覆盖。"
      footer={
        <>
          {renamed && (
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setDraft('')}>
              恢复原标题
            </Button>
          )}
          <span className="flex-1" />
          <Button size="sm" variant="ghost" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button size="sm" disabled={busy || (!renamed && draft.trim() === '')} onClick={submit}>
            保存
          </Button>
        </>
      }
      onClose={onClose}
    >
      <div className="flex flex-col gap-2">
        {!renamed && (
          <p className="text-xs text-muted">客户端标题：{current || '（无标题）'}</p>
        )}
        <Input
          autoFocus
          value={draft}
          placeholder={renamed ? '' : '输入新的显示名…'}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && draft.trim()) void submit();
          }}
        />
      </div>
    </Dialog>
  );
}

/** 标签编辑：现有标签点选、新标签回车添加，保存=全量替换。 */
export function TagsDialog({
  keyId,
  current,
  allTags,
  onClose,
  onDone,
}: {
  keyId: string;
  current: string[];
  allTags: string[];
  onClose: () => void;
  onDone: () => void;
}) {
  const [draft, setDraft] = useState<string[]>(current);
  const [input, setInput] = useState('');
  const [busy, setBusy] = useState(false);

  const toggle = (tag: string) =>
    setDraft((prev) => (prev.includes(tag) ? prev.filter((t) => t !== tag) : [...prev, tag]));

  const addFromInput = () => {
    const tag = input.trim();
    if (tag && !draft.includes(tag)) setDraft((prev) => [...prev, tag]);
    setInput('');
  };

  const submit = async () => {
    setBusy(true);
    const ok = (await runCmd('session_tag_set', { key: keyId, tags: draft })) !== undefined;
    setBusy(false);
    if (!ok) return;
    toast(draft.length ? `已设置 ${draft.length} 个标签` : '已清空标签');
    onDone();
    onClose();
  };

  return (
    <Dialog
      open
      title="会话标签"
      subtitle="标签只存在本工具里；同一会话可挂多个，筛选下拉按标签过滤。"
      footer={
        <>
          <span className="flex-1" />
          <Button size="sm" variant="ghost" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button size="sm" disabled={busy} onClick={submit}>
            保存
          </Button>
        </>
      }
      onClose={onClose}
    >
      <div className="flex flex-col gap-3">
        {(draft.length > 0 || allTags.length > 0) && (
          <div className="flex flex-wrap gap-1.5">
            {draft
              .filter((tag) => !allTags.includes(tag))
              .map((tag) => (
                <Chip key={tag} label={tag} active onClick={() => toggle(tag)} />
              ))}
            {allTags.map((tag) => (
              <Chip key={tag} label={tag} active={draft.includes(tag)} onClick={() => toggle(tag)} />
            ))}
          </div>
        )}
        <div className="flex items-center gap-2">
          <Input
            autoFocus
            className="flex-1"
            value={input}
            placeholder="输入标签，回车添加…"
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                addFromInput();
              }
            }}
          />
          <Button size="sm" variant="ghost" disabled={!input.trim()} onClick={addFromInput}>
            添加
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function Chip({ label, active, onClick }: { label: string; active: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      className={`cursor-pointer rounded-full border px-2.5 py-0.5 text-xs ${
        active ? 'border-accent bg-accent/10 text-fg' : 'border-line text-muted hover:bg-hover hover:text-fg'
      }`}
      onClick={onClick}
    >
      {label}
    </button>
  );
}

export interface SuggestItem {
  key: string;
  /** 客户端原标题（多半是「（未命名会话）」占位） */
  current: string;
  /** 建议标题（首条用户消息截短），可编辑 */
  suggestion: string;
  /** 是否采纳这一条 */
  checked: boolean;
}

/** 未命名会话批量治理：后端出建议（首条用户消息），人工确认后批量写入本地命名。 */
export function SuggestTitlesDialog({
  items: initial,
  onClose,
  onDone,
}: {
  items: SuggestItem[];
  onClose: () => void;
  onDone: (applied: number) => void;
}) {
  const [items, setItems] = useState<SuggestItem[]>(initial);
  const [busy, setBusy] = useState(false);
  const checkedCount = useMemo(() => items.filter((i) => i.checked && i.suggestion.trim()).length, [items]);

  const submit = async () => {
    setBusy(true);
    let applied = 0;
    for (const item of items) {
      const title = item.suggestion.trim();
      if (!item.checked || !title) continue;
      const ok = (await runCmd('session_rename', { key: item.key, title })) !== undefined;
      if (ok) applied += 1;
    }
    setBusy(false);
    toast(`已为 ${applied} 条未命名会话设置本地命名`);
    onDone(applied);
    onClose();
  };

  return (
    <Dialog
      open
      wide
      title="为未命名会话生成标题建议"
      subtitle="建议取自每条会话的第一句提问；确认后才写入，不满意可以改。"
      footer={
        <>
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => setItems((prev) => prev.map((i) => ({ ...i, checked: true })))}
          >
            全选
          </Button>
          <span className="flex-1" />
          <Button size="sm" variant="ghost" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button size="sm" disabled={busy || checkedCount === 0} onClick={submit}>
            应用（{checkedCount}）
          </Button>
        </>
      }
      onClose={onClose}
    >
      <div className="flex flex-col gap-1">
        {items.length === 0 && (
          <p className="py-4 text-center text-sm text-muted">这些会话没能解析出第一句提问，先去客户端里聊两句再来。</p>
        )}
        {items.map((item, index) => (
          <div key={item.key} className="flex items-center gap-2 border-b border-line py-1.5 last:border-none">
            <Checkbox
              checked={item.checked}
              onChange={() =>
                setItems((prev) => prev.map((i, i2) => (i2 === index ? { ...i, checked: !i.checked } : i)))
              }
            />
            <span className="w-40 min-w-0 shrink-0 truncate text-xs text-muted" title={item.current}>
              {item.current || '（无标题）'}
            </span>
            <Input
              className="min-w-0 flex-1 text-sm"
              value={item.suggestion}
              onChange={(e) =>
                setItems((prev) => prev.map((i, i2) => (i2 === index ? { ...i, suggestion: e.target.value } : i)))
              }
            />
          </div>
        ))}
      </div>
    </Dialog>
  );
}

/** 拉标题建议：keys → 建议列表（拿不到建议的会话跳过）。 */
export async function fetchSuggestions(
  keys: string[],
): Promise<SuggestItem[] | null> {
  const suggestions = await runCmd('session_suggest_titles', { keys });
  if (suggestions === undefined) return null;
  return suggestions.map(([key, title]) => ({
    key,
    current: '',
    suggestion: title,
    checked: true,
  }));
}
