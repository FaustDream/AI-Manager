// 「从软件收进技能库」对话框：选来源 → 勾选可导入项 → 批量入库。
// 从 page.tsx 拆出（主页面过长）；交互与行为不变。
import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { Tag } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, Select, StateBlock } from '@/components/ui/form';
import { runCmd } from '@/lib/cmd';
import { toast } from '@/components/ui/toast';
import type { HubSnapshot } from '@/lib/types';

export function ImportDialog({
  open,
  onClose,
  targets,
}: {
  open: boolean;
  onClose: () => void;
  targets: HubSnapshot['targets'];
}) {
  const qc = useQueryClient();
  const [source, setSource] = useState('');
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [importing, setImporting] = useState(false);

  // 打开时自动落到「有可导入技能」的那个平台，并默认勾上全部
  useEffect(() => {
    if (!open) return;
    const first =
      targets.find((t) => t.installed.some((s) => !s.in_library)) ?? targets[0];
    setSource(first?.key ?? '');
    setPicked(new Set(first?.installed.filter((s) => !s.in_library).map((s) => s.name) ?? []));
  }, [open, targets]);

  const current = targets.find((t) => t.key === source);
  const skills = current?.installed ?? [];
  const importable = skills.filter((s) => !s.in_library);
  const allPicked = importable.length > 0 && importable.every((s) => picked.has(s.name));

  const switchTarget = (key: string) => {
    setSource(key);
    const next = targets.find((t) => t.key === key);
    setPicked(new Set(next?.installed.filter((s) => !s.in_library).map((s) => s.name) ?? []));
  };

  const toggle = (name: string) => {
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  };

  const runImport = async () => {
    setImporting(true);
    try {
      const report = await runCmd('hub_skill_import_many', {
        target: source,
        names: [...picked],
      });
      if (report === undefined) return;
      toast(
        report.errors.length
          ? `${report.imported.length} 个已入库，${report.errors.length} 个失败：${report.errors[0]}`
          : `已导入技能库：${report.imported.join('、')}`,
      );
      qc.invalidateQueries({ queryKey: ['hub'] });
      onClose();
    } finally {
      setImporting(false);
    }
  };

  return (
    <Dialog
      open={open}
      title="从软件收进技能库"
      subtitle="把各软件里已有、但库里没有的技能收进来，之后就能统一分发"
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose}>取消</Button>
          <Button
            variant="primary"
            loading={importing}
            disabled={picked.size === 0 || importing}
            onClick={() => void runImport()}
          >
            导入 {picked.size} 个
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-sm">
        <label className="flex items-center gap-2">
          来源软件
          <Select value={source} onChange={(e) => switchTarget(e.target.value)}>
            {targets.map((t) => {
              const count = t.installed.filter((s) => !s.in_library).length;
              return (
                <option key={t.key} value={t.key}>
                  {t.label}
                  {count > 0 ? `（${count} 个可导入）` : '（无可导入）'}
                </option>
              );
            })}
          </Select>
        </label>

        {skills.length === 0 ? (
          <StateBlock
            kind="empty"
            title="这个软件没有技能目录"
            detail="它可能还没装过技能，或者路径不存在。"
          />
        ) : (
          <>
            <div className="flex items-center justify-between text-xs text-muted">
              <span>
                {importable.length} 个可导入，{skills.length - importable.length} 个已在库里
              </span>
              <button
                type="button"
                className="cursor-pointer text-accent hover:underline disabled:text-muted disabled:no-underline"
                disabled={importable.length === 0}
                onClick={() =>
                  setPicked(allPicked ? new Set() : new Set(importable.map((s) => s.name)))
                }
              >
                {allPicked ? '全不选' : '全选'}
              </button>
            </div>
            <div className="max-h-64 overflow-auto rounded-md border border-line">
              {skills.map((skill) => (
                <label
                  key={skill.name}
                  className={`flex items-center gap-2 border-b border-line px-2.5 py-1.5 last:border-b-0 ${
                    skill.in_library ? 'opacity-55' : 'cursor-pointer hover:bg-hover'
                  }`}
                >
                  <Checkbox
                    checked={skill.in_library || picked.has(skill.name)}
                    disabled={skill.in_library}
                    onChange={() => toggle(skill.name)}
                  />
                  <span className="min-w-0 flex-1 truncate">{skill.name}</span>
                  {skill.in_library && <Tag tone="muted">已在库</Tag>}
                </label>
              ))}
            </div>
            <p className="text-xs text-muted">
              已在库的同名技能会被跳过，不会覆盖库里现有版本。
            </p>
          </>
        )}
      </div>
    </Dialog>
  );
}
