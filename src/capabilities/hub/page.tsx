// 技能与 MCP 分发：技能库部署/移除/导入、MCP 粘贴导入/移除、平台目标一览。
// 冲突不静默覆盖：deploy 需要 overwrite 确认，后端已兜底，前端给「覆盖」入口。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { call, errText } from '@/lib/ipc';
import { fmtSize } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { Notice, Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import type { DeployReport, HubSnapshot, Step } from '@/lib/types';

function TargetPicker({
  targets,
  picked,
  onToggle,
  onAll,
}: {
  targets: HubSnapshot['targets'];
  picked: Set<string>;
  onToggle: (key: string) => void;
  onAll: (all: boolean) => void;
}) {
  const all = targets.length > 0 && picked.size === targets.length;
  return (
    <div className="flex flex-wrap items-center gap-2">
      {/* 全选属于对话框里的平台选择，不是技能表的行选择 */}
      <label className="flex items-center gap-1 text-sm">
        <Checkbox checked={all} onChange={() => onAll(!all)} />
        全选
      </label>
      {targets.map((t) => (
        <label key={t.key} className="flex items-center gap-1 text-sm">
          <Checkbox checked={picked.has(t.key)} onChange={() => onToggle(t.key)} />
          {t.label}
        </label>
      ))}
    </div>
  );
}

function reportText(steps: Step[]): string {
  return steps
    .map((s) => `${s.target}：${s.message}${s.conflict ? '（冲突，未覆盖）' : ''}`)
    .join('\n');
}

export default function HubPage() {
  const qc = useQueryClient();
  const hub = useQuery<HubSnapshot>({
    queryKey: ['hub'],
    queryFn: () => call<HubSnapshot>('hub_snapshot'),
  });

  const [deployName, setDeployName] = useState<string | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  // 视图标签：'all' = 总体（技能库 + 全部 MCP），其余为平台 key
  const [tab, setTab] = useState<string>('all');
  // 对话框动作（部署/移除/写 MCP）的加载态
  const [busy, setBusy] = useState<string | null>(null);
  const runBusy = (key: string, work: () => Promise<void>) => {
    if (busy) return;
    setBusy(key);
    void work().finally(() => setBusy(null));
  };
  const [importPick, setImportPick] = useState(false);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [mcpText, setMcpText] = useState('');
  const [mcpTargets, setMcpTargets] = useState<Set<string>>(new Set());

  const toggle = (set: Set<string>, key: string, apply: (s: Set<string>) => void) => {
    const next = new Set(set);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    apply(next);
  };

  const refresh = () => qc.invalidateQueries({ queryKey: ['hub'] });

  const deploy = (overwrite: boolean) =>
    runBusy('deploy', async () => {
      const name = deployName!;
      try {
        const report = await call<DeployReport>('hub_skill_deploy', {
          name,
          targets: [...picked],
          overwrite,
        });
        toast(reportText(report.steps) || '没有选择平台');
        setDeployName(null);
        refresh();
      } catch (e) {
        toast(errText(e));
      }
    });

  const importMcp = () =>
    runBusy('mcp', async () => {
      if (!mcpText.trim() || mcpTargets.size === 0) return;
      try {
        const r = await call<{ names: string[]; report: DeployReport }>('hub_mcp_import', {
          text: mcpText,
          targets: [...mcpTargets],
        });
        toast(`写入 ${r.names.join('、')}\n${reportText(r.report.steps)}`);
        setMcpOpen(false);
        setMcpText('');
        refresh();
      } catch (e) {
        toast(errText(e));
      }
    });

  const removeSkill = () =>
    runBusy('remove', async () => {
      try {
        const report = await call<DeployReport>('hub_skill_remove', {
          name: deployName,
          targets: [...picked],
        });
        toast(reportText(report.steps));
        setDeployName(null);
        refresh();
      } catch (e) {
        toast(errText(e));
      }
    });

  const removeMcp = (name: string, keys: string[]) =>
    runBusy(`mcp-remove:${name}`, async () => {
      try {
        const report = await call<DeployReport>('hub_mcp_remove', { name, targets: keys });
        toast(reportText(report.steps) || '先在右侧勾选平台');
        refresh();
      } catch (e) {
        toast(errText(e));
      }
    });

  // 平台视图里的单平台移除：不进部署对话框，确认即删
  const removeSkillFrom = (name: string, key: string) =>
    runBusy(`remove:${name}:${key}`, async () => {
      try {
        const report = await call<DeployReport>('hub_skill_remove', { name, targets: [key] });
        toast(reportText(report.steps));
        refresh();
      } catch (e) {
        toast(errText(e));
      }
    });

  if (hub.isLoading) return <SkeletonRows rows={4} cols={3} />;
  if (hub.isError) {
    return <StateBlock kind="error" title="读技能库失败" detail={errText(hub.error)} onRetry={() => hub.refetch()} />;
  }

  const snap = hub.data!;
  const conflicts = snap.skills.filter((s) => s.states.some(([, state]) => state === 'stale'));
  const activeTarget = snap.targets.find((t) => t.key === tab);
  const STATE_LABEL: Record<string, string> = { synced: '最新', stale: '待更新', missing: '未部署' };

  return (
    <div className="flex flex-col gap-3">
      {/* 总体 = 技能库 + 全部 MCP；单个平台 = 该平台的技能副本与它自己的 MCP 配置 */}
      <div className="flex flex-wrap items-center gap-1 border-b border-line pb-2">
        <button
          className={`rounded-md px-3 py-1 text-sm ${
            tab === 'all' ? 'bg-hover font-semibold text-fg' : 'text-muted hover:bg-hover hover:text-fg'
          }`}
          onClick={() => setTab('all')}
        >
          总体（技能库 {snap.skills.length} · MCP {snap.mcp.length}）
        </button>
        {snap.targets.map((t) => (
          <button
            key={t.key}
            className={`rounded-md px-3 py-1 text-sm ${
              tab === t.key ? 'bg-hover font-semibold text-fg' : 'text-muted hover:bg-hover hover:text-fg'
            }`}
            onClick={() => setTab(t.key)}
          >
            {t.label}（{t.skills}）
          </button>
        ))}
      </div>

      {tab === 'all' && (
        <>
          {conflicts.length > 0 && (
            <Notice tone="warn">
              {conflicts.length} 个技能的平台副本与库不一致（启动自动同步会在下次启动时刷新已部署的平台）。
            </Notice>
          )}

          <div className="flex flex-wrap items-center gap-2 text-sm">
            <Button variant="primary" onClick={() => setMcpOpen(true)}>
              粘贴导入 MCP
            </Button>
            <span className="text-xs text-muted">勾选平台后可对 MCP 条目做移除。</span>
          </div>

          <div className="rounded-lg border border-line bg-card p-3">
            <div className="mb-2 flex items-center justify-between">
              <h2 className="text-sm font-semibold">技能库（{snap.skills.length}）</h2>
              {/* 图例：点阵颜色不单独表意（悬停有完整文字提示） */}
              <span className="flex items-center gap-3 text-xs text-muted">
                <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-ok" />最新</span>
                <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-warn" />待更新</span>
                <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-line-strong" />未部署</span>
              </span>
            </div>
            <table className="w-full border-collapse">
              <thead>
                <tr>
                  <th className="border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">技能</th>
                  <th className="border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">说明</th>
                  <th className="w-20 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">大小</th>
                  <th className="w-44 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">平台副本状态</th>
                  <th className="w-40 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">操作</th>
                </tr>
              </thead>
              <tbody>
                {snap.skills.length === 0 && (
                  <tr>
                    <td colSpan={5}>
                      <StateBlock kind="empty" title="技能库是空的" detail="把技能目录放进 data/skills/，或从平台导入。" />
                    </td>
                  </tr>
                )}
                {snap.skills.map((skill) => (
                  <tr key={skill.name} className="group hover:bg-hover">
                    <td className="border-b border-line px-2 py-1.5 text-sm">
                      {skill.name}
                      {!skill.valid && <Tag tone="bad">{skill.reason}</Tag>}
                    </td>
                    <td className="border-b border-line px-2 py-1.5 text-xs text-muted">
                      {skill.description}
                    </td>
                    <td className="border-b border-line px-2 py-1.5 text-xs tabular-nums text-muted">
                      {fmtSize(skill.size)}
                    </td>
                    {/* 状态点阵：列序与平台标签栏一致，点一个跳对应平台页 */}
                    <td className="border-b border-line px-2 py-1.5">
                      <span className="flex items-center gap-2">
                        {snap.targets.map((t) => {
                          const state = skill.states.find(([k]) => k === t.key)?.[1] ?? 'missing';
                          return (
                            <button
                              key={t.key}
                              className={`h-2.5 w-2.5 cursor-pointer rounded-full transition-transform hover:scale-125 ${
                                state === 'synced'
                                  ? 'bg-ok'
                                  : state === 'stale'
                                    ? 'bg-warn ring-2 ring-warn/30'
                                    : 'bg-line-strong group-hover:bg-muted'
                              }`}
                              title={`${t.label}：${STATE_LABEL[state] ?? state}`}
                              onClick={() => setTab(t.key)}
                            />
                          );
                        })}
                      </span>
                    </td>
                    <td className="border-b border-line px-2 py-1.5">
                      <span className="flex gap-1">
                        <Button size="sm" onClick={() => { setDeployName(skill.name); setPicked(new Set()); }}>
                          部署
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => setImportPick(true)}>
                          从平台导入
                        </Button>
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="rounded-lg border border-line bg-card p-3">
            <h2 className="mb-2 text-sm font-semibold">MCP（{snap.mcp.length}）</h2>
            <table className="w-full border-collapse">
              <thead>
                <tr>
                  <th className="border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">名称</th>
                  <th className="border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">命令</th>
                  <th className="w-24 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">状态</th>
                  <th className="border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">已写入平台</th>
                  <th className="w-24 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">操作</th>
                </tr>
              </thead>
              <tbody>
                {snap.mcp.length === 0 && (
                  <tr>
                    <td colSpan={5}>
                      <StateBlock kind="empty" title="还没有 MCP 条目" detail="点「粘贴导入 MCP」把配置写进选中的平台。" />
                    </td>
                  </tr>
                )}
                {snap.mcp.map((row) => (
                  <tr key={row.name} className="hover:bg-hover">
                    <td className="border-b border-line px-2 py-1.5 text-sm">
                      {row.name}
                      {row.foreign && <Tag tone="muted">非本工具写入</Tag>}
                    </td>
                    <td className="border-b border-line px-2 py-1.5 font-mono text-xs text-muted">{row.command}</td>
                    <td className="border-b border-line px-2 py-1.5">
                      <Tag tone={row.disabled ? 'warn' : 'ok'}>{row.disabled ? '停用' : '启用'}</Tag>
                    </td>
                    <td className="border-b border-line px-2 py-1.5 text-xs text-muted">
                      {row.targets.join('、') || '—'}
                    </td>
                    <td className="border-b border-line px-2 py-1.5">
                      {!row.foreign && (
                        <Button
                          size="sm"
                          variant="danger"
                          onClick={() => removeMcp(row.name, [...picked])}
                          loading={busy === `mcp-remove:${row.name}`}
                          disabled={!!busy || picked.size === 0}
                        >
                          移除
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      {activeTarget && (
        <>
          <div className="rounded-lg border border-line bg-card p-3 text-sm">
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
              <span className="font-semibold">{activeTarget.label}</span>
              <span className="text-xs text-muted">技能目录 {activeTarget.skills} 个 · MCP {activeTarget.mcp} 条</span>
            </div>
            <div className="mt-1 truncate text-xs text-muted" title={activeTarget.skill_dir}>
              技能目录：{activeTarget.skill_dir}
            </div>
            <div className="truncate text-xs text-muted" title={activeTarget.mcp_file}>
              MCP 配置：{activeTarget.mcp_file}
            </div>
          </div>

          <div className="rounded-lg border border-line bg-card p-3">
            <h2 className="mb-2 text-sm font-semibold">这个平台的技能（{snap.skills.length}）</h2>
            <table className="w-full border-collapse">
              <thead>
                <tr>
                  <th className="border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">技能</th>
                  <th className="w-24 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">副本状态</th>
                  <th className="w-48 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">操作</th>
                </tr>
              </thead>
              <tbody>
                {snap.skills.length === 0 && (
                  <tr>
                    <td colSpan={3}>
                      <StateBlock kind="empty" title="技能库是空的" detail="回「总体」页把技能放进 data/skills/ 或从其他平台导入。" />
                    </td>
                  </tr>
                )}
                {snap.skills.map((skill) => {
                  const state = skill.states.find(([k]) => k === tab)?.[1] ?? 'missing';
                  return (
                    <tr key={skill.name} className="hover:bg-hover">
                      <td className="border-b border-line px-2 py-1.5 text-sm">
                        {skill.name}
                        {!skill.valid && <Tag tone="bad">{skill.reason}</Tag>}
                        <span className="block text-xs text-muted">{skill.description}</span>
                      </td>
                      <td className="border-b border-line px-2 py-1.5">
                        <Tag tone={state === 'synced' ? 'ok' : state === 'stale' ? 'warn' : 'muted'}>
                          {STATE_LABEL[state] ?? state}
                        </Tag>
                      </td>
                      <td className="border-b border-line px-2 py-1.5">
                        <span className="flex gap-1">
                          {(state === 'missing' || state === 'stale') && (
                            <Button
                              size="sm"
                              onClick={() => {
                                setDeployName(skill.name);
                                setPicked(new Set([tab]));
                              }}
                            >
                              {state === 'stale' ? '更新此副本' : '部署到此平台'}
                            </Button>
                          )}
                          {state !== 'missing' && (
                            <Button
                              size="sm"
                              variant="danger"
                              loading={busy === `remove:${skill.name}:${tab}`}
                              disabled={!!busy}
                              onClick={() => {
                                if (!window.confirm(`从${activeTarget.label}移除「${skill.name}」？副本会先进回收站。`)) return;
                                void removeSkillFrom(skill.name, tab);
                              }}
                            >
                              移除
                            </Button>
                          )}
                        </span>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          <div className="rounded-lg border border-line bg-card p-3">
            <h2 className="mb-2 text-sm font-semibold">这个平台的 MCP（{snap.mcp.filter((m) => m.present.includes(tab)).length}）</h2>
            <table className="w-full border-collapse">
              <thead>
                <tr>
                  <th className="border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">名称</th>
                  <th className="border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">命令</th>
                  <th className="w-20 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">状态</th>
                  <th className="w-32 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">来源</th>
                  <th className="w-20 border-b border-line px-2 py-1 text-left text-xs font-semibold text-muted">操作</th>
                </tr>
              </thead>
              <tbody>
                {snap.mcp.filter((m) => m.present.includes(tab)).length === 0 && (
                  <tr>
                    <td colSpan={5}>
                      <StateBlock kind="empty" title="这个平台还没有 MCP 条目" detail="回「总体」页点「粘贴导入 MCP」。" />
                    </td>
                  </tr>
                )}
                {snap.mcp
                  .filter((m) => m.present.includes(tab))
                  .map((row) => (
                    <tr key={row.name} className="hover:bg-hover">
                      <td className="border-b border-line px-2 py-1.5 text-sm">{row.name}</td>
                      <td className="border-b border-line px-2 py-1.5 font-mono text-xs text-muted">{row.command}</td>
                      <td className="border-b border-line px-2 py-1.5">
                        <Tag tone={row.disabled ? 'warn' : 'ok'}>{row.disabled ? '停用' : '启用'}</Tag>
                      </td>
                      <td className="border-b border-line px-2 py-1.5 text-xs text-muted">
                        {row.targets.includes(tab) ? '本工具部署' : '非本工具写入'}
                      </td>
                      <td className="border-b border-line px-2 py-1.5">
                        {!row.foreign && (
                          <Button
                            size="sm"
                            variant="danger"
                            onClick={() => removeMcp(row.name, [tab])}
                            loading={busy === `mcp-remove:${row.name}`}
                            disabled={!!busy}
                          >
                            移除
                          </Button>
                        )}
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      {/* 部署对话框：选平台 → 无冲突直接写；冲突时给「覆盖」确认（契约 16） */}
      <Dialog
        open={deployName !== null}
        title={`部署「${deployName ?? ''}」到平台`}
        onClose={() => setDeployName(null)}
        footer={
          <>
            <Button onClick={() => setDeployName(null)} disabled={!!busy}>
              取消
            </Button>
            <Button
              variant="primary"
              disabled={picked.size === 0 || !!busy}
              loading={busy === 'deploy'}
              onClick={() => deploy(false)}
            >
              部署
            </Button>
          </>
        }
      >
        <TargetPicker
          targets={snap.targets}
          picked={picked}
          onToggle={(key) => toggle(picked, key, setPicked)}
          onAll={(all) =>
            setPicked(all ? new Set(snap.targets.map((t) => t.key)) : new Set())
          }
        />
        <p className="mt-2 text-xs text-muted">
          目标已有同名但内容不同的副本时会报冲突，需要你在结果里确认覆盖。
        </p>
        <div className="mt-2 flex gap-2">
          <Button
            size="sm"
            variant="danger"
            onClick={() => deploy(true)}
            disabled={picked.size === 0 || !!busy}
            loading={busy === 'deploy'}
          >
            强制覆盖部署
          </Button>
          <Button
            size="sm"
            variant="ghost"
            disabled={picked.size === 0 || !!busy}
            loading={busy === 'remove'}
            onClick={removeSkill}
          >
            从选中平台移除
          </Button>
        </div>
      </Dialog>

      <ImportDialog open={importPick} onClose={() => setImportPick(false)} targets={snap.targets} />

      <Dialog
        open={mcpOpen}
        title="粘贴导入 MCP"
        onClose={() => setMcpOpen(false)}
        wide
        footer={
          <>
            <Button onClick={() => setMcpOpen(false)} disabled={!!busy}>
              取消
            </Button>
            <Button
              variant="primary"
              onClick={importMcp}
              disabled={!mcpText.trim() || mcpTargets.size === 0 || !!busy}
              loading={busy === 'mcp'}
            >
              写入选中平台
            </Button>
          </>
        }
      >
        <p className="mb-2 text-xs text-muted">
          支持标准 mcpServers、ZCode 的 mcp.servers、单条目、数组、裸对象。写前会自动备份原配置。
        </p>
        <textarea
          className="h-40 w-full rounded-md border border-line bg-bg p-2 font-mono text-xs"
          value={mcpText}
          onChange={(e) => setMcpText(e.target.value)}
          placeholder='{"mcpServers": {"github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"]}}}'
        />
        <div className="mt-2">
          <TargetPicker
            targets={snap.targets}
            picked={mcpTargets}
            onToggle={(key) => toggle(mcpTargets, key, setMcpTargets)}
            onAll={(all) =>
              setMcpTargets(all ? new Set(snap.targets.map((t) => t.key)) : new Set())
            }
          />
        </div>
      </Dialog>
    </div>
  );
}

function ImportDialog({
  open,
  onClose,
  targets,
}: {
  open: boolean;
  onClose: () => void;
  targets: HubSnapshot['targets'];
}) {
  const qc = useQueryClient();
  const [source, setSource] = useState(targets[0]?.key ?? '');
  const [name, setName] = useState('');
  const [importing, setImporting] = useState(false);

  return (
    <Dialog
      open={open}
      title="从平台导入技能"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>取消</Button>
          <Button
            variant="primary"
            loading={importing}
            disabled={!name.trim() || importing}
            onClick={async () => {
              setImporting(true);
              try {
                const info = await call<{ name: string }>('hub_skill_import', { target: source, name });
                toast(`已导入 ${info.name}`);
                qc.invalidateQueries({ queryKey: ['hub'] });
                onClose();
              } catch (e) {
                toast(errText(e));
              } finally {
                setImporting(false);
              }
            }}
          >
            导入
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2 text-sm">
        <label className="flex items-center gap-2">
          平台
          <Select value={source} onChange={(e) => setSource(e.target.value)}>
            {targets.map((t) => (
              <option key={t.key} value={t.key}>
                {t.label}
              </option>
            ))}
          </Select>
        </label>
        <label className="flex items-center gap-2">
          技能目录名
          <input
            className="flex-1 rounded-sm border border-line bg-bg px-2 py-1 text-sm"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="平台技能目录里的目录名（含 SKILL.md）"
          />
        </label>
        <p className="text-xs text-muted">库里已有同名技能会被拒绝，不会覆盖。</p>
      </div>
    </Dialog>
  );
}
