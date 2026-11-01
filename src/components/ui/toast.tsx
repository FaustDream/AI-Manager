import { create } from 'zustand';
import { AnimatePresence, motion } from 'motion/react';

// 全局 toast：跨页通知（替代 v1 的 bus）。文本可能多行，按 pre-wrap 渲染。
// Motion 遵守系统「减少动画」设置（MotionConfig 在 Shell 挂全局）。
export interface ToastItem {
  id: number;
  text: string;
}

interface ToastStore {
  items: ToastItem[];
  push: (text: string) => void;
  dismiss: (id: number) => void;
}

let nextId = 1;

export const useToasts = create<ToastStore>((set) => ({
  items: [],
  push: (text) => {
    const id = nextId++;
    set((s) => ({ items: [...s.items.slice(-4), { id, text }] }));
    setTimeout(() => set((s) => ({ items: s.items.filter((t) => t.id !== id) })), 5000);
  },
  dismiss: (id) => set((s) => ({ items: s.items.filter((t) => t.id !== id) })),
}));

export function toast(text: string): void {
  useToasts.getState().push(text);
}

export function ToastHost() {
  const items = useToasts((s) => s.items);
  return (
    <div className="fixed bottom-14 right-5 z-50 flex flex-col gap-2">
      <AnimatePresence initial={false}>
        {items.map((item) => (
          <motion.div
            key={item.id}
            layout
            initial={{ opacity: 0, y: 8, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 4, transition: { duration: 0.12 } }}
            transition={{ duration: 0.16, ease: 'easeOut' }}
            className="max-w-[520px] cursor-pointer whitespace-pre-wrap rounded-lg border border-line bg-card px-4 py-2.5 shadow-lg"
            onClick={() => useToasts.getState().dismiss(item.id)}
          >
            {item.text}
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
