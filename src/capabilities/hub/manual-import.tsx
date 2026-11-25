// 手动导入技能：把用户自己准备的技能放进分发库。
//
// 与「从软件导入」并列的另一个入口——那条路是从别人那里收，这条路是自己给。
// 两种形态：本地技能目录（整份复制）/ 粘贴 SKILL.md（落成一个技能）。
import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { Tabs } from '@/components/ui';
import { Dialog } from '@/components/ui/dialog';
import { Input, Textarea } from '@/components/ui/form';
import { runCmd } from '@/lib/cmd';
import { toast } from '@/components/ui/toast';

const TEMPLATE = `---
name: my-skill
description: 一句话说明这个技能做什么
---

# my-skill

## 什么时候用

## 怎么做
`;

export function ManualSkillDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const [mode, setMode] = useState<'folder' | 'paste'>('folder');
  const [path, setPath] = useState('');
  const [name, setName] = useState('');
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);

  const finish = async (skillName: string) => {
    toast(`已加入分发库：${skillName}`);
    await qc.invalidateQueries({ queryKey: ['hub'] });
    setPath('');
    setName('');
    setText('');
    onClose();
  };

  const submit = async () => {
    setBusy(true);
    try {
      if (mode === 'folder') {
        const info = await runCmd('hub_skill_import_local', { path: path.trim() });
        if (info === undefined) return;
        await finish(info.name);
      } else {
        const info = await runCmd('hub_skill_create', { name: name.trim(), text });
        if (info === undefined) return;
        await finish(info.name);
      }
    } finally {
      setBusy(false);
    }
  };

  const ready = mode === 'folder' ? path.trim().length > 0 : name.trim().length > 0 && text.trim().length > 0;

  return (
    <Dialog
      open={open}
      title="手动导入技能"
      subtitle="把自己准备的技能放进分发库，之后就能部署到任意软件"
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            取消
          </Button>
          <Button variant="primary" loading={busy} disabled={!ready || busy} onClick={() => void submit()}>
            {mode === 'folder' ? '导入这个目录' : '创建技能'}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-sm">
        <Tabs
          label="导入方式"
          size="sm"
          variant="solid"
          value={mode}
          className="self-start rounded-md border border-line bg-sunken p-0.5"
          onValueChange={(next) => setMode(next as 'folder' | 'paste')}
          items={[
            { value: 'folder', label: '本地技能目录' },
            { value: 'paste', label: '粘贴 SKILL.md' },
          ]}
        />

        {mode === 'folder' ? (
          <>
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted">技能目录的完整路径</span>
              <Input
                value={path}
                onChange={(e) => setPath(e.target.value)}
                placeholder="D:\my-skills\pdf-reader"
                className="font-mono"
              />
            </label>
            <p className="text-xs text-muted">
              目录根上要有 SKILL.md（可以带子目录和脚本，整份复制）。技能名取 SKILL.md 里声明的
              name，没写就用目录名；库里已有同名技能时不会覆盖。
            </p>
          </>
        ) : (
          <>
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted">技能名（会作为技能目录名）</span>
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="my-skill"
                className="font-mono"
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted">SKILL.md 正文</span>
              <Textarea
                value={text}
                onChange={(e) => setText(e.target.value)}
                placeholder={TEMPLATE}
                className="h-48 font-mono text-xs"
              />
            </label>
            <div className="flex items-center justify-between text-xs text-muted">
              <span>没写 frontmatter 会自动补上 name，各软件才认得出这个技能。</span>
              <button
                type="button"
                className="cursor-pointer text-accent hover:underline"
                onClick={() => setText(TEMPLATE)}
              >
                填入模板
              </button>
            </div>
          </>
        )}
      </div>
    </Dialog>
  );
}
