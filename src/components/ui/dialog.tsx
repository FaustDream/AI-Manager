import { useEffect } from 'react';
import type { ReactNode } from 'react';
import { cn } from './button';

// 原生 <dialog> 的轻包装：模态、点背景不关（防误触），Esc 允许关。
export function Dialog({
  open,
  title,
  children,
  footer,
  onClose,
  wide,
}: {
  open: boolean;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  wide?: boolean;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-[rgba(0,0,0,.45)]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className={cn(
          'max-h-[85vh] min-w-[380px] overflow-auto rounded-lg border border-line bg-bg p-5 shadow-xl',
          wide && 'min-w-[520px]',
        )}
      >
        <h3 className="mb-3 text-md font-semibold">{title}</h3>
        {children}
        {footer && <div className="mt-4 flex justify-end gap-2">{footer}</div>}
      </div>
    </div>
  );
}
