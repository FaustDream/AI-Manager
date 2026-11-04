import { create } from 'zustand';
import { Dialog } from './dialog';
import { Button } from './button';

// 不可逆动作的统一确认接缝：话术、危险色、按钮文案一处维护。
// 界面 confirmAction() 等一个布尔值，不再在 window.confirm 与自绘 Dialog 之间二选一。
export interface ConfirmSpec {
  title: string;
  message: string;
  /** 不可逆动作用 danger 主按钮 */
  danger?: boolean;
  okText?: string;
}

interface ConfirmStore {
  spec: ConfirmSpec | null;
  resolve: ((ok: boolean) => void) | null;
  ask: (spec: ConfirmSpec) => Promise<boolean>;
  answer: (ok: boolean) => void;
}

export const useConfirm = create<ConfirmStore>((set, get) => ({
  spec: null,
  resolve: null,
  ask: (spec) => new Promise<boolean>((resolve) => set({ spec, resolve })),
  answer: (ok) => {
    get().resolve?.(ok);
    set({ spec: null, resolve: null });
  },
}));

export function confirmAction(spec: ConfirmSpec): Promise<boolean> {
  return useConfirm.getState().ask(spec);
}

export function ConfirmHost() {
  const spec = useConfirm((s) => s.spec);
  const answer = useConfirm((s) => s.answer);
  return (
    <Dialog
      open={!!spec}
      title={spec?.title ?? ''}
      onClose={() => answer(false)}
      footer={
        <>
          <Button size="sm" variant="ghost" onClick={() => answer(false)}>
            取消
          </Button>
          <Button
            size="sm"
            variant={spec?.danger ? 'danger' : 'primary'}
            onClick={() => answer(true)}
          >
            {spec?.okText ?? '确定'}
          </Button>
        </>
      }
    >
      <p className="whitespace-pre-wrap text-sm leading-6">{spec?.message}</p>
    </Dialog>
  );
}
