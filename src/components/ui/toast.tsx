import { create } from 'zustand';
import { AnimatePresence, motion } from 'motion/react';

// 全局 toast：跨页通知（替代 v1 的 bus）。文本可能多行，按 pre-wrap 渲染。
// Motion 遵守系统「减少动画」设置（MotionConfig 在 Shell 挂全局）。
//
// 分层（总纲 P0-2）：`text` 只放人话 + 下一步，`detail` 放 kind / 参数 / 原文这类
// 排障字段，默认折起。新手读第一行就够，要深挖的人点「详情」——两类读者不再互相干扰。
export interface ToastItem {
  id: number;
  text: string;
  /** 技术证据（失败时来自 failureSummary）。默认折叠，不影响正文可读性。 */
  detail?: string;
}

interface ToastStore {
  items: ToastItem[];
  push: (text: string, durationMs?: number, detail?: string) => void;
  dismiss: (id: number) => void;
}

let nextId = 1;

/** 带详情的 toast 给更长阅读时间：点开详情再读，5s 常常不够。 */
const DETAIL_TTL = 12_000;

export const useToasts = create<ToastStore>((set) => ({
  items: [],
  push: (text, durationMs, detail) => {
    const id = nextId++;
    set((s) => ({ items: [...s.items.slice(-4), { id, text, detail }] }));
    // 导出路径这类长文本给更长的阅读时间；默认 5s 不变
    const ttl = durationMs ?? (detail ? DETAIL_TTL : 5000);
    setTimeout(() => set((s) => ({ items: s.items.filter((t) => t.id !== id) })), ttl);
  },
  dismiss: (id) => set((s) => ({ items: s.items.filter((t) => t.id !== id) })),
}));

export function toast(text: string, durationMs?: number, detail?: string): void {
  useToasts.getState().push(text, durationMs, detail);
}

export function ToastHost() {
  const items = useToasts((s) => s.items);
  return (
    // aria-live：toast 是「动作做完了没有」的唯一即时反馈，读屏必须能听到（P0-6）。
    // polite 而非 assertive：不打断用户正在读的内容。
    <div
      className="fixed bottom-14 right-5 z-50 flex flex-col gap-2"
      role="status"
      aria-live="polite"
      aria-atomic="false"
    >
      <AnimatePresence initial={false}>
        {items.map((item) => (
          <motion.div
            key={item.id}
            layout
            initial={{ opacity: 0, y: 8, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 4, transition: { duration: 0.12 } }}
            transition={{ duration: 0.16, ease: 'easeOut' }}
            className="max-w-[520px] cursor-pointer rounded-lg border border-line bg-card px-4 py-2.5 shadow-card"
            onClick={() => useToasts.getState().dismiss(item.id)}
          >
            <div className="whitespace-pre-wrap">{item.text}</div>
            {item.detail && (
              // 展开详情不该顺手把 toast 关掉：整块卡片点击即消失，这里必须挡住冒泡
              <details
                className="mt-1.5 text-xs text-muted"
                onClick={(event) => event.stopPropagation()}
              >
                <summary className="cursor-pointer select-none hover:text-fg">详情</summary>
                <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] leading-4">
                  {item.detail}
                </pre>
              </details>
            )}
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
