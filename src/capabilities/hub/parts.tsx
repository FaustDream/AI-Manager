// hub 的展示件：平台选择器（已部署/已存在的平台锁定）与子标签页。
// 从 page.tsx 拆出——纯展示、无业务状态，没有理由和页面状态机挤在一个文件里。
import { Checkbox } from '@/components/ui/form';
import type { HubSnapshot } from '@/lib/types';

export function TargetPicker({
  targets,
  picked,
  onToggle,
  onAll,
  locked,
  lockedLabel,
  hints,
}: {
  targets: HubSnapshot['targets'];
  picked: Set<string>;
  onToggle: (key: string) => void;
  /** 只回传可选项的 key：已部署的平台不参与全选 */
  onAll: (keys: string[]) => void;
  /** 已部署的平台：显示为已勾选且不可再选，避免重复部署 */
  locked?: Set<string>;
  /** 锁定项的角标文案（技能用「已部署」、MCP 用「已存在」） */
  lockedLabel?: string;
  /** 未锁定但需要提示的平台（例如「待更新」） */
  hints?: Record<string, string>;
}) {
  const selectable = targets.filter((t) => !locked?.has(t.key));
  const pickedCount = selectable.filter((t) => picked.has(t.key)).length;
  const all = selectable.length > 0 && pickedCount === selectable.length;
  return (
    <div>
      {/* 全选属于对话框里的平台选择，不是技能表的行选择 */}
      <label className="flex items-center gap-1.5 text-xs text-muted">
        <Checkbox checked={all} onChange={() => onAll(all ? [] : selectable.map((t) => t.key))} />
        全选（{pickedCount}/{selectable.length}）
      </label>
      {/* 双列选项卡：点击面大、选中态描边，比一排小 checkbox 好点也好扫 */}
      <div className="mt-2 grid grid-cols-2 gap-2">
        {targets.map((t) => {
          const isLocked = locked?.has(t.key) ?? false;
          const checked = isLocked || picked.has(t.key);
          return (
            <label
              key={t.key}
              className={`flex items-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors ${
                isLocked
                  ? 'cursor-not-allowed border-line opacity-70'
                  : `cursor-pointer ${checked ? 'border-accent bg-hover font-medium' : 'border-line hover:bg-hover'}`
              }`}
            >
              <Checkbox checked={checked} disabled={isLocked} onChange={() => onToggle(t.key)} />
              <span className="min-w-0 flex-1 truncate">{t.label}</span>
              {isLocked && <span className="shrink-0 text-[10px] text-muted">{lockedLabel ?? '已部署'}</span>}
              {!isLocked && hints?.[t.key] && (
                <span className="shrink-0 text-[10px] text-warn">{hints[t.key]}</span>
              )}
            </label>
          );
        })}
      </div>
    </div>
  );
}

export function SubTabs({
  value,
  onChange,
  items,
}: {
  value: string;
  onChange: (v: string) => void;
  items: { key: string; label: string }[];
}) {
  return (
    <div className="flex items-center gap-1">
      {items.map((it) => (
        <button
          key={it.key}
          className={`rounded-md px-2.5 py-1 text-xs transition-colors ${
            value === it.key ? 'bg-accent font-medium text-on-accent' : 'text-muted hover:bg-hover hover:text-fg'
          }`}
          onClick={() => onChange(it.key)}
        >
          {it.label}
        </button>
      ))}
    </div>
  );
}
