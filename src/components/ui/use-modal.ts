// 浮层共用纪律：焦点进面板、Tab 在面板内循环、关闭后归还给触发它的元素，
// 以及「先画完退场再拆节点」。弹窗与抽屉共用这一份——两处各写一遍必然走偏。
import { useEffect, useRef, useState } from 'react';

const FOCUSABLE =
  'a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';

/** 退场留白：与 .overlay-out / .view-exit 的时长一致（多留会让人以为按钮点不动）。 */
const EXIT_MS = 120;

export function useModal(open: boolean, onClose: () => void) {
  const panel = useRef<HTMLDivElement | null>(null);
  // onClose 基本都是调用方现写的箭头函数。**绝不能进 effect 依赖**：
  // 那样父组件每次重渲染（例如在弹窗里打一个字）都会重跑效应、把焦点拽回第一个元素，
  // 输入框直接没法连续输入。用 ref 读最新值，效应只跟 open 走。
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  const [visible, setVisible] = useState(open);
  useEffect(() => {
    if (open) {
      setVisible(true);
      return;
    }
    if (!visible) return;
    const timer = window.setTimeout(() => setVisible(false), EXIT_MS);
    return () => window.clearTimeout(timer);
  }, [open, visible]);

  useEffect(() => {
    if (!open) return;
    // 记住是谁打开的：关闭后把焦点还回去，键盘用户不会掉到页面顶部
    const opener = document.activeElement as HTMLElement | null;
    const node = panel.current;
    const items = () => Array.from(node?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    items()[0]?.focus();

    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        closeRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const list = items();
      if (list.length === 0) return;
      const first = list[0];
      const last = list[list.length - 1];
      const active = document.activeElement;
      // 首尾循环：Shift+Tab 从第一个绕到最后一个，Tab 从最后一个绕回第一个
      if (event.shiftKey && (active === first || !node?.contains(active))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
      // 触发者可能已经被卸载（例如确认后那一行被删了），isConnected 兜一层
      if (opener?.isConnected) opener.focus();
    };
  }, [open]);

  return { panel, visible, closing: visible && !open };
}