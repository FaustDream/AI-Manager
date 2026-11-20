// 命令面板（⌘K / Ctrl+K）：页面直达 + 常用动作。
//
// 只做「跳转」和「触发已有命令」，不在这里发明新能力——面板一旦能执行
// 别处没有的操作，就会出现「只有快捷键用户才知道」的隐藏功能。
import { useEffect, useMemo, useRef, useState } from 'react';
import { cn } from '@/components/ui/button';

export type PaletteItem = {
  id: string;
  title: string;
  group: '页面' | '动作' | '会话';
  hint?: string;
  run: () => void;
};

/** 多词 AND + 忽略大小写：输入「账号 中心」也能命中「账号中心」。 */
function matches(text: string, query: string): boolean {
  if (!query.trim()) return true;
  const haystack = text.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

export function CommandPalette({ items, placeholder }: { items: PaletteItem[]; placeholder?: string }) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  const filtered = useMemo(
    () => items.filter((item) => matches(`${item.title} ${item.hint ?? ''} ${item.group}`, query)),
    [items, query],
  );

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const combo = (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k';
      if (combo) {
        event.preventDefault();
        setOpen((value) => !value);
        setQuery('');
        setIndex(0);
        return;
      }
      if (!open) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        setOpen(false);
      }
    };
    // 顶栏「搜索 ⌘K」按钮走这个事件（P1-12）：面板自己管开合，
    // 壳层不必把 open 提升成受控状态——与 aim:navigate 同一套约定。
    const onOpen = () => {
      setOpen(true);
      setQuery('');
      setIndex(0);
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('aim:palette', onOpen);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('aim:palette', onOpen);
    };
  }, [open]);

  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  useEffect(() => {
    setIndex(0);
  }, [query]);

  // 方向键走到视口外的条目要跟着滚，否则「选中了但看不见」
  useEffect(() => {
    if (!open) return;
    listRef.current?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [index, open]);

  if (!open) return null;

  const runAt = (position: number) => {
    const item = filtered[position];
    if (!item) return;
    setOpen(false);
    item.run();
  };

  const optionId = (position: number) => `aim-palette-opt-${position}`;

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-[12vh]"
      onClick={() => setOpen(false)}
    >
      <div
        className="w-[min(560px,92vw)] overflow-hidden rounded-lg border border-line bg-card shadow-card-hover"
        onClick={(event) => event.stopPropagation()}
      >
        <input
          ref={inputRef}
          value={query}
          placeholder={placeholder ?? '搜索页面或动作…（Esc 关闭）'}
          className="w-full border-b border-line bg-transparent px-3 py-2.5 text-sm outline-none focus-visible:outline-none"
          // combobox 语义（P2-18）：读屏据此报「组合框，已展开，当前第 N 项」。
          role="combobox"
          aria-expanded="true"
          aria-controls="aim-palette-list"
          aria-autocomplete="list"
          aria-activedescendant={filtered.length > 0 ? optionId(index) : undefined}
          aria-label="搜索页面、动作与会话"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'ArrowDown') {
              event.preventDefault();
              setIndex((value) => Math.min(value + 1, filtered.length - 1));
            } else if (event.key === 'ArrowUp') {
              event.preventDefault();
              setIndex((value) => Math.max(value - 1, 0));
            } else if (event.key === 'Enter') {
              event.preventDefault();
              runAt(index);
            }
          }}
        />
        <div
          ref={listRef}
          id="aim-palette-list"
          role="listbox"
          aria-label="搜索结果"
          className="max-h-[52vh] overflow-auto py-1"
        >
          {filtered.length === 0 && (
            <p className="px-3 py-4 text-center text-xs text-muted">没有匹配的页面或动作。</p>
          )}
          {filtered.map((item, position) => (
            <button
              key={item.id}
              id={optionId(position)}
              data-index={position}
              role="option"
              aria-selected={position === index}
              // 焦点始终留在输入框（combobox 模式），条目本身不进 Tab 序列
              tabIndex={-1}
              className={cn(
                'flex w-full cursor-pointer items-center gap-2 px-3 py-1.5 text-left text-sm',
                position === index ? 'bg-hover' : 'hover:bg-hover',
              )}
              onMouseEnter={() => setIndex(position)}
              onClick={() => runAt(position)}
            >
              <span className="w-8 text-[10px] text-muted">{item.group}</span>
              <span className="flex-1 truncate">{item.title}</span>
              {item.hint && <span className="text-xs text-muted">{item.hint}</span>}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-3 border-t border-line px-3 py-1.5 text-[11px] text-muted">
          <span>↑↓ 选择</span>
          <span>Enter 执行</span>
          <span>Esc 关闭</span>
          <span className="flex-1" />
          <span>⌘K / Ctrl+K</span>
        </div>
      </div>
    </div>
  );
}
