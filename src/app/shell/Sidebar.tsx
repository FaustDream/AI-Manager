// 侧边栏：品牌区 + 菜单 + 折叠开关。
//
// 两种宽度共用一套结构：240px 展开（图标 + 文字）与 56px 图标轨道
// （只留图标，说明进 title 气泡）。折叠只改宽度与文字显隐，**不换组件树**——
// 换树会让折叠那一下丢掉焦点与滚动位置。
//
// 选中态三件事一起给：淡主色底 + 左侧指示条 + 图标转主色。
// 只靠底色在四套风格里深浅不一，指示条是那条在所有风格下都读得出的锚。
//
// 拖拽排序：指针事件 + 位移阈值（4px 才进入拖动），所以「点一下切页」和
// 「按住拖」共用同一个按钮；拖动时源项变成虚线占位块，其余项用 FLIP 滑动让位，
// 落点按拖动开始时各菜单项的中线计算，避免占位块把指针下的目标推来推去。
// 不引入拖拽库：列表最多十几项，自实现比引一个包更容易控制样式与手感。
import type { Capability } from '@/app/router';
import { cn } from '@/components/ui/button';
import { reorderIds } from '@/lib/ui-state';
import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { navIcon } from './nav-icons';
import { PanelLeftClose, PanelLeftOpen } from 'lucide-react';

/** 超过这个位移才算拖动，否则当普通点击——手抖不该把菜单顺序改掉。 */
const DRAG_THRESHOLD = 4;

export function Sidebar({
  caps,
  active,
  collapsed,
  onGo,
  onToggle,
  onReorder,
}: {
  caps: Capability[];
  active: string;
  collapsed: boolean;
  onGo: (id: string) => void;
  onToggle: () => void;
  onReorder: (ids: string[]) => void;
}) {
  const [dragging, setDragging] = useState<string | null>(null);
  const [dropAt, setDropAt] = useState<{ id: string; before: boolean } | null>(null);
  // 落点同时记一份 ref：onUp 里直接读，避免把提交顺序的副作用塞进 setState 更新函数
  const dropRef = useRef<{ id: string; before: boolean } | null>(null);
  const listRef = useRef<HTMLUListElement>(null);
  // FLIP 用 offsetTop（不受 transform 影响），记录上一帧每个菜单项的版式位置
  const prevTops = useRef<Map<string, number>>(new Map());
  const animations = useRef<Map<string, Animation>>(new Map());
  const dragRef = useRef<{
    id: string;
    x: number;
    y: number;
    active: boolean;
    ghost: HTMLDivElement | null;
    raf: number;
    // 拖动开始时各菜单项的中线：预览换位不改变这些值，避免落点跟着动画来回抖
    rows: { id: string; mid: number }[];
  } | null>(null);
  // 拖动结束浏览器还会补一次 click：用它拦掉「拖完顺手导航」。
  // 下一次 pointerdown 会重置，不会误吞后续的正常点击。
  const draggedRef = useRef(false);
  // 回调走 ref：拖动监听器是在 pointerdown 那一刻建的，闭包里的 props 可能已过期
  const reorderRef = useRef(onReorder);
  reorderRef.current = onReorder;

  // 拖动中的预览顺序：把源项插到当前落点，其余项实时让位。
  // 松手时写回同一结果，所以拖动过程中看到的就是最终位置。
  const preview = useMemo(() => {
    if (!dragging || !dropAt) return caps;
    const dragged = caps.find((cap) => cap.manifest.id === dragging);
    if (!dragged) return caps;
    const rest = caps.filter((cap) => cap.manifest.id !== dragging);
    const at = rest.findIndex((cap) => cap.manifest.id === dropAt.id);
    if (at < 0) return caps;
    const next = [...rest];
    next.splice(dropAt.before ? at : at + 1, 0, dragged);
    return next;
  }, [caps, dragging, dropAt]);

  // FLIP：列表换位后，其余项先从旧位置反向位移，再滑回 0。
  // 用 offsetTop（不受 transform 影响）读版式位置，动画叠加时也不会互相污染。
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const next = new Map<string, number>();
    for (const item of list.querySelectorAll<HTMLLIElement>('li[data-nav-id]')) {
      const id = item.dataset.navId!;
      const top = item.offsetTop;
      next.set(id, top);
      const prev = prevTops.current.get(id);
      // 占位块跟手跳，不参与缓动；其余项让位时才有滑动
      if (id === dragging || prev === undefined) continue;
      const dy = prev - top;
      if (Math.abs(dy) < 1) continue;
      animations.current.get(id)?.cancel();
      const anim = item.animate(
        [{ transform: `translateY(${dy}px)` }, { transform: 'translateY(0)' }],
        { duration: 180, easing: 'cubic-bezier(0.2, 0.8, 0.2, 1)' },
      );
      animations.current.set(id, anim);
    }
    prevTops.current = next;
  });

  const startDrag = (e: React.PointerEvent<HTMLButtonElement>, id: string) => {
    if (e.button !== 0 || caps.length < 2) return;
    // 以拖动开始时的版式为准计算落点：拖动过程中列表会实时让位，
    // 若每帧都读当前 rect，占位块会把指针下的目标推来推去，产生抖动。
    const rows: { id: string; mid: number }[] = [];
    for (const item of listRef.current?.querySelectorAll<HTMLLIElement>('li[data-nav-id]') ?? []) {
      const rowId = item.dataset.navId!;
      if (rowId === id) continue;
      const box = item.getBoundingClientRect();
      rows.push({ id: rowId, mid: box.top + box.height / 2 });
    }
    dragRef.current = { id, x: e.clientX, y: e.clientY, active: false, ghost: null, raf: 0, rows };
    dropRef.current = null;
    setDropAt(null);
    draggedRef.current = false;

    const moveGhost = (ev: PointerEvent) => {
      const drag = dragRef.current;
      if (drag?.ghost) {
        drag.ghost.style.transform = `translate(${ev.clientX + 12}px, ${ev.clientY + 8}px) scale(1.04)`;
      }
    };

    const onMove = (ev: PointerEvent) => {
      const drag = dragRef.current;
      if (!drag) return;
      if (!drag.active) {
        if (
          Math.abs(ev.clientX - drag.x) < DRAG_THRESHOLD &&
          Math.abs(ev.clientY - drag.y) < DRAG_THRESHOLD
        ) {
          return;
        }
        // 真正进入拖动：源项变成虚线占位块，另起一个跟随指针的浮层标签
        drag.active = true;
        draggedRef.current = true;
        setDragging(drag.id);
        const ghost = document.createElement('div');
        ghost.textContent = caps.find((cap) => cap.manifest.id === drag.id)?.manifest.title ?? '';
        ghost.style.cssText =
          'position:fixed;z-index:60;pointer-events:none;padding:2px 10px;border-radius:6px;' +
          'background:var(--color-card);border:1px solid var(--color-line-strong);' +
          'font:600 12px var(--font-sans);color:var(--color-fg);box-shadow:0 8px 24px rgb(0 0 0 / 0.22);' +
          'will-change:transform';
        ghost.style.transform = `translate(${ev.clientX + 12}px, ${ev.clientY + 8}px) scale(1.04)`;
        document.body.appendChild(ghost);
        // 浮起动画：从贴近原位置的尺寸淡入，明确「拿起来了」
        ghost.animate(
          [
            { opacity: 0, scale: 0.92 },
            { opacity: 1, scale: 1 },
          ],
          { duration: 140, easing: 'cubic-bezier(0.2, 0.8, 0.2, 1)' },
        );
        drag.ghost = ghost;
        window.addEventListener('pointermove', moveGhost, true);
      }
      // rAF 节流：一帧最多算一次落点
      if (drag.raf) return;
      drag.raf = requestAnimationFrame(() => {
        if (!dragRef.current) return;
        dragRef.current.raf = 0;
        if (drag.rows.length === 0) return;
        // 指针落在哪两项中线之间：第一个中线在指针下方的项就是插入点；
        // 都在上方表示放到最后一项之后。
        const first = drag.rows.find((row) => ev.clientY < row.mid);
        const target = first ?? drag.rows[drag.rows.length - 1];
        const before = Boolean(first);
        const next = { id: target.id, before };
        dropRef.current = next;
        setDropAt((old) => (old?.id === next.id && old.before === next.before ? old : next));
      });
    };

    const onUp = () => {
      window.removeEventListener('pointermove', onMove, true);
      window.removeEventListener('pointermove', moveGhost, true);
      window.removeEventListener('pointerup', onUp, true);
      const drag = dragRef.current;
      dragRef.current = null;
      drag?.ghost?.remove();
      setDragging(null);
      const drop = dropRef.current;
      dropRef.current = null;
      setDropAt(null);
      if (!drag?.active || !drop) return;
      const next = reorderIds(
        caps.map((cap) => cap.manifest.id),
        drag.id,
        drop.id,
        drop.before,
      );
      if (next) reorderRef.current(next);
    };

    window.addEventListener('pointermove', onMove, true);
    window.addEventListener('pointerup', onUp, true);
  };

  return (
    <aside
      className={cn(
        'relative z-20 flex shrink-0 flex-col border-r border-line bg-bg',
        'transition-[width] duration-[var(--dur-3)] ease-[var(--ease-std)]',
        collapsed ? 'w-14' : 'w-60',
      )}
    >
      {/* 品牌区：渐变方块取自当前风格的三个色相，换肤时它跟着变 */}
      <div className={cn('flex h-14 items-center gap-2.5 border-b border-line', collapsed ? 'justify-center px-2' : 'px-4')}>
        <span
          aria-hidden
          className="grid h-8 w-8 shrink-0 place-items-center rounded-md text-xs font-bold text-on-accent shadow-card"
          style={{
            backgroundImage:
              'linear-gradient(135deg, var(--color-accent), var(--color-iris) 55%, var(--color-info))',
          }}
        >
          AI
        </span>
        {!collapsed && (
          <span className="block truncate text-md leading-5 font-semibold tracking-tight">AI Manager</span>
        )}
      </div>

      <nav className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden py-3" aria-label="主导航">
        {/* select-none：拖动过程中不让浏览器顺带选中整个侧栏的文字 */}
        <ul
          ref={listRef}
          className={cn('flex select-none flex-col gap-0.5', collapsed ? 'px-2' : 'px-2.5')}
        >
          {preview.map((cap) => {
            const Icon = navIcon(cap.manifest.id);
            const on = active === cap.manifest.id;
            // 快捷键编号按稳定顺序算，拖动预览期间不乱跳
            const at = caps.findIndex((item) => item.manifest.id === cap.manifest.id) + 1;
            const isDragging = dragging === cap.manifest.id;
            return (
              <li key={cap.manifest.id} data-nav-id={cap.manifest.id} className="relative rounded-md">
                {isDragging ? (
                  // 虚线占位块：让「会落到这里」变成看得见的空位，而不是一条细线
                  <div
                    aria-hidden
                    className={cn(
                      'flex h-9 items-center justify-center rounded-md border border-dashed border-accent bg-accent-soft text-[11px] font-medium text-accent',
                      collapsed ? 'px-0' : 'px-2.5',
                    )}
                  >
                    {collapsed ? <span className="h-1.5 w-1.5 rounded-full bg-accent" /> : '松手放到这里'}
                  </div>
                ) : (
                  <button
                    type="button"
                    onPointerDown={(e) => startDrag(e, cap.manifest.id)}
                    onClick={() => {
                      if (draggedRef.current) {
                        draggedRef.current = false;
                        return;
                      }
                      onGo(cap.manifest.id);
                    }}
                    aria-current={on ? 'page' : undefined}
                    // 图标轨道里没有可见文字，可读名字必须显式给（title 只是兜底）
                    aria-label={cap.manifest.title}
                    title={
                      collapsed
                        ? `${cap.manifest.title}（快捷键 ${at}，可拖动排序）${cap.manifest.note ? ` — ${cap.manifest.note}` : ''}`
                        : `${cap.manifest.note ?? cap.manifest.title}（快捷键 ${at}，可拖动排序）`
                    }
                    className={cn(
                      'group relative flex w-full items-center gap-2.5 rounded-md py-2 text-sm',
                      dragging ? 'cursor-grabbing' : 'cursor-pointer',
                      'transition-colors duration-[var(--dur-1)]',
                      collapsed ? 'justify-center px-0' : 'px-2.5',
                      on
                        ? 'bg-accent-soft font-medium text-fg'
                        : 'text-fg2 hover:bg-hover hover:text-fg',
                    )}
                  >
                    {on && (
                      <span
                        aria-hidden
                        className="nav-marker absolute top-1/2 left-0 h-5 w-[3px] -translate-y-1/2 rounded-r-full bg-accent"
                      />
                    )}
                    <Icon
                      size={16}
                      className={cn(
                        'shrink-0 transition-colors duration-[var(--dur-1)]',
                        on ? 'text-accent' : 'text-muted group-hover:text-fg2',
                      )}
                    />
                    {!collapsed && <span className="min-w-0 truncate">{cap.manifest.title}</span>}
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      </nav>

      {/* 页脚只留折叠开关：图标即可自解释，文字说明不再占位 */}
      <div className="border-t border-line px-2 py-2">
        <button
          type="button"
          onClick={onToggle}
          aria-label={collapsed ? '展开侧边栏' : '收起侧边栏'}
          title={collapsed ? '展开侧边栏' : '收起为图标轨道'}
          className={cn(
            'flex w-full cursor-pointer items-center justify-center rounded-md px-2 py-1.5 text-xs text-muted',
            'transition-colors duration-[var(--dur-1)] hover:bg-hover hover:text-fg',
          )}
        >
          {collapsed ? <PanelLeftOpen size={15} /> : <PanelLeftClose size={15} />}
        </button>
      </div>
    </aside>
  );
}
