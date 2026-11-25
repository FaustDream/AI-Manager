// 顶栏风格快捷菜单：四套视觉风格 + 跟随系统，一眼看到色带就能选，不用进设置。
// 与设置页共用 lib/theme 的同一份快照——两处切换永远显示同一个当前值。
import { useEffect, useRef, useState } from 'react';
import { Check, Monitor, Palette } from 'lucide-react';
import { cn } from '@/components/ui/button';
import { Hint } from '@/components/ui/tooltip';
import { saveTheme, THEME_STYLES, useThemeChoice, type ThemeChoice } from '@/lib/theme';

export function ThemeMenu() {
  const choice = useThemeChoice();
  const [open, setOpen] = useState(false);
  const host = useRef<HTMLDivElement>(null);
  const items = useRef<(HTMLButtonElement | null)[]>([]);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (!host.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        setOpen(false);
      }
    };
    window.addEventListener('mousedown', onDown);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const options: { key: ThemeChoice; name: string; note: string; swatch?: readonly string[] }[] = [
    { key: '', name: '跟随系统', note: '浅色用简约商务，深色用暗色科技' },
    ...THEME_STYLES,
  ];
  const currentName = choice ? THEME_STYLES.find((s) => s.key === choice)?.name : '跟随系统';

  const pick = (key: ThemeChoice) => {
    void saveTheme(key);
    setOpen(false);
  };

  return (
    <div className="relative shrink-0" ref={host}>
      <Hint content={`视觉风格：${currentName}`} focusable={false}>
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={`视觉风格：${currentName}，点击切换`}
          className={cn(
            'flex cursor-pointer items-center gap-1.5 rounded-md border border-line px-2 py-1 text-sm text-fg2',
            'transition-colors duration-[var(--dur-1)] hover:bg-hover hover:text-fg',
            open && 'bg-hover text-fg',
          )}
        >
          <Palette size={14} className="text-accent" />
          <span className="hidden xl:inline">{currentName}</span>
        </button>
      </Hint>

      {open && (
        <div
          role="menu"
          aria-label="视觉风格"
          className={cn(
            'dialog-in absolute top-full right-0 z-50 mt-1.5 w-64 overflow-hidden rounded-lg border border-line',
            'bg-card p-1 shadow-pop',
          )}
          onKeyDown={(event) => {
            const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
            if (!step) return;
            event.preventDefault();
            const list = items.current.filter(Boolean) as HTMLButtonElement[];
            const at = list.indexOf(document.activeElement as HTMLButtonElement);
            list[(at + step + list.length) % list.length]?.focus();
          }}
        >
          {options.map((option, index) => {
            const on = option.key === choice;
            return (
              <button
                key={option.key || 'system'}
                ref={(el) => {
                  items.current[index] = el;
                }}
                type="button"
                role="menuitemradio"
                aria-checked={on}
                onClick={() => pick(option.key)}
                className={cn(
                  'flex w-full cursor-pointer items-center gap-2.5 rounded-md px-2 py-2 text-left',
                  'transition-colors duration-[var(--dur-1)]',
                  on ? 'bg-accent-soft' : 'hover:bg-hover',
                )}
              >
                {option.swatch ? (
                  <span
                    aria-hidden
                    className="flex h-7 w-7 shrink-0 overflow-hidden rounded-md border border-line"
                  >
                    {option.swatch.map((color, at) => (
                      <span key={at} className="h-full flex-1" style={{ background: color }} />
                    ))}
                  </span>
                ) : (
                  <span className="grid h-7 w-7 shrink-0 place-items-center rounded-md border border-line text-muted">
                    <Monitor size={14} />
                  </span>
                )}
                <span className="min-w-0 flex-1">
                  <span className={cn('block truncate text-sm', on ? 'font-medium text-fg' : 'text-fg')}>
                    {option.name}
                  </span>
                  <span className="block truncate text-xs text-muted">{option.note}</span>
                </span>
                {on && <Check size={14} className="shrink-0 text-accent" />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}