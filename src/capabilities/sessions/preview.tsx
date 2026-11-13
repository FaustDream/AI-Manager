// 会话正文渲染：超长单条折叠 + 渐进挂载 + 复制全文。
// 活跃页与「已归档会话」页共用，避免两套预览慢慢不一致。
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { toast } from '@/components/ui/toast';
import type { Message } from '@/lib/types';

const PREVIEW_CHUNK = 50;
const FOLD_LIMIT = 3000;
const FOLD_KEEP = 2000;

/** 超长单条：默认只显示前 FOLD_KEEP 字，展开/收起由用户点。 */
export function Foldable({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  if (text.length <= FOLD_LIMIT) {
    return <div className="whitespace-pre-wrap break-words">{text}</div>;
  }
  return (
    <div className="whitespace-pre-wrap break-words">
      {open ? text : `${text.slice(0, FOLD_KEEP)}\n…（余 ${text.length - FOLD_KEEP} 字）`}
      <button
        className="ml-2 text-xs text-accent hover:underline"
        onClick={() => setOpen(!open)}
      >
        {open ? '收起' : '展开'}
      </button>
    </div>
  );
}

/** 渐进渲染：先挂 PREVIEW_CHUNK 条，「展开」按批追加。切换会话时重置。 */
export function PreviewBody({ messages }: { messages: Message[] }) {
  const [shown, setShown] = useState(PREVIEW_CHUNK);
  useEffect(() => setShown(PREVIEW_CHUNK), [messages]);
  if (messages.length === 0) {
    return <p className="text-sm text-muted">没有可读的消息。</p>;
  }
  return (
    <>
      {messages.slice(0, shown).map((msg, i) => (
        <div
          key={i}
          className={`mb-2 border-l-2 pl-2 text-sm ${
            msg.role === 'user' ? 'border-l-accent' : 'border-l-ok'
          }`}
        >
          <div className="text-[11px] tracking-wide text-muted">{msg.role}</div>
          <Foldable text={msg.text} />
        </div>
      ))}
      {shown < messages.length && (
        <button
          className="mt-1 rounded-md border border-line px-3 py-1 text-xs hover:bg-hover"
          onClick={() => setShown((n) => n + PREVIEW_CHUNK)}
        >
          展开剩余 {messages.length - shown} 条
        </button>
      )}
    </>
  );
}

/** 复制全文：折叠与渐进都不影响复制——复制的是全量消息。 */
export function CopyAllButton({ messages }: { messages: Message[] }) {
  const [done, setDone] = useState(false);
  return (
    <Button
      size="sm"
      variant="ghost"
      disabled={messages.length === 0}
      onClick={async () => {
        const text = messages.map((m) => `${m.role}: ${m.text}`).join('\n\n');
        await navigator.clipboard.writeText(text).catch(() => {});
        setDone(true);
        toast(`已复制 ${messages.length} 条消息到剪贴板`);
        setTimeout(() => setDone(false), 2000);
      }}
    >
      {done ? '已复制' : '复制全文'}
    </Button>
  );
}
