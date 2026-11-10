// hub 的展示件：平台选择器（已部署/已存在的平台锁定）、子标签页、来源标签。
// 从 page.tsx 拆出——纯展示、无业务状态，没有理由和页面状态机挤在一个文件里。
import { Checkbox } from '@/components/ui/form';
import { Tag } from '@/components/ui';
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

/**
 * 来源标签：个人技能 / 官方（收录自某软件 / 软件自带）。
 *
 * 需求要求两类来源不能混在一个无标识的列表里，所以每条都带一个标签：
 * 个人用 accent 描边，官方一律灰色并写明出处。
 */
export function SourceTag({
  kind,
  from,
  label,
}: {
  /** manual = 个人；official = 收录自某软件；builtin = 软件自带（不在分发库里） */
  kind: 'manual' | 'official' | 'builtin';
  /** 收录来源：平台键（界面自己换成平台名） */
  from?: string;
  /** 平台键 → 显示名 */
  label?: (key: string) => string;
}) {
  if (kind === 'builtin') {
    return (
      <Tag tone="muted" className="shrink-0" title="这个软件自带或你自己放进来的，不在分发库里">
        自带
      </Tag>
    );
  }
  if (kind === 'official') {
    const name = from ? (label?.(from) ?? from) : '某软件';
    return (
      <Tag tone="muted" className="shrink-0" title={`从 ${name} 的官方技能收录进来`}>
        收录·{name}
      </Tag>
    );
  }
  return (
    <Tag tone="accent" className="shrink-0" title="手动导入或自行创建的个人技能">
      个人
    </Tag>
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
