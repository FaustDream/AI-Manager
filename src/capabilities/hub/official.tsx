// 官方自带收录：一次扫全部平台，把「平台自带的技能」与「非本工具写入的 MCP」
// 勾选收进来。写入全部复用既有命令——技能走 hub_skill_import_many（按平台分组），
// MCP 把原始条目包成标准 {"mcpServers": …} 文本走 hub_mcp_import，后端不设第二套写入。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { Tag } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, StateBlock } from '@/components/ui/form';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { toast } from '@/components/ui/toast';
import type { HubSnapshot, OfficialScan } from '@/lib/types';
import { TargetPicker } from './parts';

export function OfficialDialog({
  open,
  onClose,
  targets,
}: {
  open: boolean;
  onClose: () => void;
  targets: HubSnapshot['targets'];
}) {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const scan = useQuery<OfficialScan>({
    queryKey: ['official-scan'],
    queryFn: () => queryCmd('hub_official_scan'),
    enabled: open,
  });
  // 勾选键：技能 `target|name`，MCP `source|name`（不同平台可有同名条目）
  const [skillPicked, setSkillPicked] = useState<Set<string>>(new Set());
  const [mcpPicked, setMcpPicked] = useState<Set<string>>(new Set());
  // MCP 收录目标：原始条目来自某个平台，写进哪些平台由用户挑
  const [mcpTargets, setMcpTargets] = useState<Set<string>>(new Set());

  const data = scan.data;

  // 每次扫描结果到达就重置为默认全选（扫描是只读的，重开对话框重新扫一遍）
  useEffect(() => {
    if (!data) return;
    setSkillPicked(new Set(data.skills.map((s) => `${s.target}|${s.name}`)));
    setMcpPicked(new Set(data.mcp.map((m) => `${m.source}|${m.name}`)));
    setMcpTargets(new Set(targets.map((t) => t.key)));
    // targets 是稳定引用（同一份 snapshot），不进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data]);

  const toggle = (set: Set<string>, apply: (next: Set<string>) => void, key: string) => {
    const next = new Set(set);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    apply(next);
  };

  const officialCount = (data?.skills.length ?? 0) + (data?.mcp.length ?? 0);

  const runImport = () =>
    run('import', async () => {
      if (!data) return;
      // 技能：按来源平台分组，一组一条批量导入
      const byTarget = new Map<string, string[]>();
      for (const s of data.skills) {
        if (!skillPicked.has(`${s.target}|${s.name}`)) continue;
        const names = byTarget.get(s.target) ?? [];
        names.push(s.name);
        byTarget.set(s.target, names);
      }
      let imported = 0;
      const errors: string[] = [];
      for (const [target, names] of byTarget) {
        const report = await runCmd('hub_skill_import_many', { target, names });
        if (report === undefined) return; // 失败已 toast，中止后续以免半途状态混乱
        imported += report.imported.length;
        errors.push(...report.errors);
      }
      // MCP：选中的条目合成一个标准配置文本，粘贴导入到勾选的平台
      const pickedMcp = data.mcp.filter((m) => mcpPicked.has(`${m.source}|${m.name}`));
      let mcpNames: string[] = [];
      if (pickedMcp.length > 0 && mcpTargets.size > 0) {
        const body = Object.fromEntries(pickedMcp.map((m) => [m.name, m.server]));
        const result = await runCmd('hub_mcp_import', {
          text: JSON.stringify({ mcpServers: body }, null, 2),
          targets: [...mcpTargets],
        });
        if (result === undefined) return;
        mcpNames = result.names;
      }
      const parts = [
        imported > 0 ? `技能入库 ${imported} 个` : '',
        mcpNames.length > 0 ? `MCP 写入 ${mcpNames.length} 条` : '',
      ].filter(Boolean);
      toast(
        parts.length
          ? `${parts.join('，')}${errors.length ? `；${errors.length} 个技能失败：${errors[0]}` : ''}`
          : '没有勾选任何可收录项',
      );
      qc.invalidateQueries({ queryKey: ['hub'] });
      onClose();
    });

  return (
    <Dialog
      open={open}
      title="收录官方自带"
      subtitle="扫描各软件自带的技能与 MCP 配置，勾选后收进技能库 / 写入选中平台"
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose}>取消</Button>
          <Button
            variant="primary"
            loading={busy === 'import'}
            disabled={officialCount === 0 || busy !== null}
            onClick={() => void runImport()}
          >
            收录勾选项
          </Button>
        </>
      }
    >
      {scan.isLoading && <p className="py-6 text-center text-sm text-muted">正在扫描各平台…</p>}
      {scan.isError && <StateBlock kind="error" title="扫描失败" detail={String(scan.error)} />}
      {data && officialCount === 0 && (
        <StateBlock kind="empty" title="没有发现可收录项" detail="各平台自带的技能都已入库，MCP 里没有非本工具写入的条目。" />
      )}
      {data && data.skills.length > 0 && (
        <section className="mb-3">
          <h3 className="mb-1.5 text-xs font-semibold text-fg2">
            平台自带技能（{data.skills.length}）
            <span className="ml-2 font-normal text-muted">收进技能库后即可统一分发</span>
          </h3>
          <div className="max-h-56 overflow-y-auto rounded-md border border-line">
            {data.skills.map((s) => {
              const key = `${s.target}|${s.name}`;
              return (
                <label
                  key={key}
                  className="flex cursor-pointer items-center gap-2 border-b border-line px-3 py-1.5 text-sm last:border-b-0 hover:bg-hover"
                >
                  <Checkbox
                    checked={skillPicked.has(key)}
                    onChange={() => toggle(skillPicked, setSkillPicked, key)}
                  />
                  <Tag tone="muted">{s.label}</Tag>
                  <span className="min-w-0 flex-1 truncate">{s.name}</span>
                </label>
              );
            })}
          </div>
        </section>
      )}
      {data && data.mcp.length > 0 && (
        <section>
          <h3 className="mb-1.5 text-xs font-semibold text-fg2">
            各平台 MCP 配置（{data.mcp.length}）
            <span className="ml-2 font-normal text-muted">勾选后写入下方选中的平台，由本工具接管</span>
          </h3>
          <div className="mb-2 max-h-40 overflow-y-auto rounded-md border border-line">
            {data.mcp.map((m) => {
              const key = `${m.source}|${m.name}`;
              return (
                <label
                  key={key}
                  className="flex cursor-pointer items-center gap-2 border-b border-line px-3 py-1.5 text-sm last:border-b-0 hover:bg-hover"
                >
                  <Checkbox
                    checked={mcpPicked.has(key)}
                    onChange={() => toggle(mcpPicked, setMcpPicked, key)}
                  />
                  <Tag tone="muted">{m.source_label}</Tag>
                  <span className="min-w-0 flex-1 truncate">{m.name}</span>
                  <span className="shrink-0 font-mono text-xs text-muted">{m.server?.command ?? ''}</span>
                </label>
              );
            })}
          </div>
          <TargetPicker
            targets={targets}
            picked={mcpTargets}
            onToggle={(key) => toggle(mcpTargets, setMcpTargets, key)}
            onAll={(keys) => setMcpTargets(new Set(keys))}
          />
        </section>
      )}
    </Dialog>
  );
}
