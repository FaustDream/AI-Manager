// 会话正文渲染：超长单条折叠 + 渐进挂载 + 复制 + role 筛选 + 命中高亮。
// 活跃页与「已归档会话」页共用，避免两套预览慢慢不一致。
import { useEffect, useMemo, useState } from 'react';
import { Button, cn } from '@/components/ui/button';
import { toast } from '@/components/ui/toast';
import { fmtTime } from '@/lib/format';
import type { Message } from '@/lib/types';

const PREVIEW_CHUNK = 50;
const FOLD_LIMIT = 3000;
const FOLD_KEEP = 2000;

/** 超长单条：默认只显示前 FOLD_KEEP 字，展开/收起由用户点。 */
export function Foldable({ text, highlight }: { text: string; highlight?: string }) {
  const [open, setOpen] = useState(false);
  const shown = open ? text : `${text.slice(0, FOLD_KEEP)}\n…（余 ${text.length - FOLD_KEEP} 字）`;
  if (text.length <= FOLD_LIMIT) {
    return (
      <div className="whitespace-pre-wrap break-words">
        <Highlight text={text} term={highlight} />
      </div>
    );
  }
  return (
    <div className="whitespace-pre-wrap break-words">
      <Highlight text={shown} term={highlight} />
      <button className="ml-2 text-xs text-accent hover:underline" onClick={() => setOpen(!open)}>
        {open ? '收起' : '展开'}
      </button>
    </div>
  );
}

/** 命中词高亮：大小写不敏感，正转义由 split 字面量完成，不上 regex。 */
export function Highlight({ text, term }: { text: string; term?: string }) {
  const needle = term?.trim();
  if (!needle) return <>{text}</>;
  const lower = text.toLowerCase();
  const target = needle.toLowerCase();
  const parts: React.ReactNode[] = [];
  let cursor = 0;
  let at = lower.indexOf(target);
  let key = 0;
  while (at !== -1) {
    if (at > cursor) parts.push(text.slice(cursor, at));
    parts.push(<mark key={key++}>{text.slice(at, at + needle.length)}</mark>);
    cursor = at + needle.length;
    at = lower.indexOf(target, cursor);
  }
  if (cursor < text.length) parts.push(text.slice(cursor));
  return <>{parts}</>;
}

type RoleFilter = 'all' | 'user' | 'assistant';

/** 渐进渲染：先挂 PREVIEW_CHUNK 条，「展开」按批追加。切换会话时重置。 */
export function PreviewBody({
  messages,
  highlight,
}: {
  messages: Message[];
  highlight?: string;
}) {
  const [shown, setShown] = useState(PREVIEW_CHUNK);
  const [role, setRole] = useState<RoleFilter>('all');
  useEffect(() => {
    setShown(PREVIEW_CHUNK);
  }, [messages]);
  useEffect(() => {
    setShown(PREVIEW_CHUNK);
  }, [role]);

  const filtered = useMemo(
    () => (role === 'all' ? messages : messages.filter((m) => m.role === role)),
    [messages, role],
  );

  if (messages.length === 0) {
    return <p className="text-sm text-muted">没有可读的消息。</p>;
  }
  return (
    <>
      <div className="mb-2 flex items-center gap-1">
        {(
          [
            ['all', '全部'],
            ['user', '用户'],
            ['assistant', 'AI'],
          ] as const
        ).map(([value, label]) => (
          <button
            key={value}
            className={cn(
              'cursor-pointer rounded px-1.5 py-0.5 text-xs',
              role === value ? 'bg-hover text-fg' : 'text-muted hover:text-fg',
            )}
            onClick={() => setRole(value)}
          >
            {label}
          </button>
        ))}
        {role !== 'all' && (
          <span className="text-xs text-muted">{filtered.length} 条</span>
        )}
      </div>
      {filtered.length === 0 && (
        <p className="text-sm text-muted">这个视图下没有消息。</p>
      )}
      {filtered.slice(0, shown).map((msg, i) => (
        <div
          key={i}
          className={`mb-2 border-l-2 pl-2 text-sm ${
            msg.role === 'user' ? 'border-l-accent' : 'border-l-ok'
          }`}
        >
          <div className="flex items-baseline gap-2 text-xs tracking-wide text-muted">
            <span>{msg.role === 'user' ? '用户' : msg.role === 'gap' ? '' : 'AI'}</span>
            {msg.time > 0 && <span className="tabular-nums">{fmtTime(msg.time)}</span>}
          </div>
          <Foldable text={msg.text} highlight={highlight} />
        </div>
      ))}
      {shown < filtered.length && (
        <button
          className="mt-1 rounded-md border border-line px-3 py-1 text-xs hover:bg-hover"
          onClick={() => setShown((n) => n + PREVIEW_CHUNK)}
        >
          展开剩余 {filtered.length - shown} 条
        </button>
      )}
    </>
  );
}

/**
 * 复制预览内容：折叠与渐进都不影响复制——复制的是**已解析**的消息。
 * 「全文」只覆盖后端解析的头尾 100 条，文案与口径对齐。
 */
export function CopyAllButton({ messages }: { messages: Message[] }) {
  const [done, setDone] = useState(false);
  const real = messages.filter((m) => m.role !== 'gap');
  return (
    <Button
      size="sm"
      variant="ghost"
      disabled={real.length === 0}
      title="复制当前预览解析出的消息（超长会话只含头尾部分）"
      onClick={async () => {
        const text = real.map((m) => `${m.role === 'user' ? '用户' : 'AI'}: ${m.text}`).join('\n\n');
        await navigator.clipboard.writeText(text).catch(() => {});
        setDone(true);
        toast(`已复制 ${real.length} 条消息到剪贴板`);
        setTimeout(() => setDone(false), 2000);
      }}
    >
      {done ? '已复制' : `复制预览（${real.length} 条）`}
    </Button>
  );
}
