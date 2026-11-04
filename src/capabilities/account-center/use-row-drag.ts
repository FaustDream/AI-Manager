// 账号表拖动排序：指针事件自实现。
//
// 为什么不用 HTML5 drag：onDragOver 每次都会整表渲染，系统拖影还会和 2s 的任务轮询
// 打架（每次返回新数组引用都重渲染，拖影直接卡住）。指针方案只在落点真正变化时
// setState，拖影走 transform，拖拽期间由调用方暂停轮询。
//
// 落点语义 = 移除源行后的插入位置；排序结果交给调用方持久化（account_reorder）。
import { useRef, useState } from 'react';
import type { AccountView } from '@/lib/types';

export interface DropTarget {
  iid: string;
  before: boolean;
}

export function useRowDrag(
  rows: AccountView[],
  busy: string | null,
  commit: (iids: string[]) => Promise<void>,
) {
  const [dragging, setDragging] = useState<string | null>(null);
  const [dropAt, setDropAt] = useState<DropTarget | null>(null);
  const dragRef = useRef<{
    iid: string;
    ghost: HTMLDivElement | null;
    lastKey: string;
    raf: number;
  } | null>(null);
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const commitRef = useRef(commit);
  commitRef.current = commit;

  const startDrag = (e: React.PointerEvent, iid: string) => {
    if (e.button !== 0 || busy) return;
    e.preventDefault();
    const ghost = document.createElement('div');
    ghost.textContent = rowsRef.current.find((r) => r.iid === iid)?.name ?? '';
    ghost.style.cssText =
      'position:fixed;z-index:60;pointer-events:none;padding:2px 10px;border-radius:6px;' +
      'background:var(--color-card);border:1px solid var(--color-line-strong);' +
      'font:600 12px var(--font-sans);color:var(--color-fg);box-shadow:0 4px 12px rgb(0 0 0 / 0.18)';
    const move = (e: PointerEvent) => {
      ghost.style.transform = `translate(${e.clientX + 12}px, ${e.clientY + 8}px)`;
    };
    ghost.style.transform = `translate(${e.clientX + 12}px, ${e.clientY + 8}px)`;
    document.body.appendChild(ghost);
    window.addEventListener('pointermove', move, true);
    dragRef.current = { iid, ghost, lastKey: iid, raf: 0 };

    const onMove = (ev: PointerEvent) => {
      // rAF 节流：一帧最多算一次落点
      if (dragRef.current?.raf) return;
      dragRef.current!.raf = requestAnimationFrame(() => {
        dragRef.current!.raf = 0;
        const row = (ev.target as HTMLElement)?.closest?.('tr[data-iid]') as HTMLTableRowElement | null;
        if (!row) return;
        const iid2 = row.dataset.iid!;
        if (iid2 === dragRef.current?.lastKey) {
          // 同一行内也要更新前后半区
          const rect = row.getBoundingClientRect();
          const before = ev.clientX - rect.left < rect.width / 2;
          setDropAt((old) =>
            old?.iid === iid2 && old.before === before ? old : { iid: iid2, before },
          );
          return;
        }
        dragRef.current!.lastKey = iid2;
        const rect = row.getBoundingClientRect();
        setDropAt({ iid: iid2, before: ev.clientX - rect.left < rect.width / 2 });
      });
    };

    const onUp = () => {
      window.removeEventListener('pointermove', onMove, true);
      window.removeEventListener('pointerup', onUp, true);
      dragRef.current?.ghost?.remove();
      const drag = dragRef.current;
      dragRef.current = null;
      setDragging(null);
      // 落点在提交前读 state：闭包里的 dropAt 可能是旧值
      setDropAt((drop) => {
        if (drag && drop && drag.iid !== drop.iid) {
          const ids = rowsRef.current.map((r) => r.iid).filter((id) => id !== drag.iid);
          const at = ids.indexOf(drop.iid);
          ids.splice(drop.before ? at : at + 1, 0, drag.iid);
          void commitRef.current(ids);
        }
        return null;
      });
    };

    window.addEventListener('pointermove', onMove, true);
    window.addEventListener('pointerup', onUp, true);
    setDragging(iid);
  };

  return { dragging, dropAt, startDrag };
}
