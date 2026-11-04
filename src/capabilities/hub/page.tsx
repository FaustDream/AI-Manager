// 技能与 MCP 分发：技能库部署/移除/导入、MCP 粘贴导入/移除、平台目标一览。
// 冲突不静默覆盖：deploy 需要 overwrite 确认，后端已兜底，前端给「覆盖」入口。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useEffect, useState } from 'react';
import { errText } from '@/lib/ipc';
import { fmtSize } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Checkbox, Select, SkeletonRows, StateBlock } from '@/components/ui/form';
import { DataTable } from '@/components/ui/table';
import { Notice, Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import type { HubSnapshot, Step } from '@/lib/types';

function TargetPicker({
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

function reportText(steps: Step[]): string {
  return steps
    .map((s) => `${s.target}：${s.message}${s.conflict ? '（冲突，未覆盖）' : ''}`)
    .join('\n');
}

// 状态矩阵列头的平台代码：3 字符内，与点阵共用同一网格严格对齐（悬停有全称）
const CODE: Record<string, string> = {
  qoder_cn: 'QCN',
  qoder_intl: 'QIN',
  trae_cn: 'TCN',
  trae_intl: 'TIN',
  workbuddy_cn: 'WCN',
  workbuddy_intl: 'WIN',
  codebuddy: 'CB',
  zcode: 'ZC',
};

/** 子标签（技能 / MCP）：与顶部主标签同款式，但更小一档。 */
function SubTabs({
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

export default function HubPage() {
  const qc = useQueryClient();
  const hub = useQuery<HubSnapshot>({
    queryKey: ['hub'],
    queryFn: () => queryCmd('hub_snapshot'),
  });

  const [deployName, setDeployName] = useState<string | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  // 视图标签：'all' = 总体（技能库 + 全部 MCP），其余为平台 key
  const [tab, setTab] = useState<string>('all');
  // 每个视图里再分技能 / MCP 两个子页
  const [pane, setPane] = useState<'skills' | 'mcp'>('skills');
  // 对话框动作（部署/移除/写 MCP）的加载态
  const { busy, run: runBusy } = useBusy();
  const [importPick, setImportPick] = useState(false);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [mcpText, setMcpText] = useState('');
  const [mcpTargets, setMcpTargets] = useState<Set<string>>(new Set());
  // MCP 行「部署」对话框：把已有条目写到更多平台
  const [mcpDeployName, setMcpDeployName] = useState<string | null>(null);
  const [mcpPicked, setMcpPicked] = useState<Set<string>>(new Set());

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
      
        const report = await runCmd('hub_skill_deploy', {
          name,
          targets: [...picked],
          overwrite,
        });

        if (report === undefined) return;
        toast(reportText(report.steps) || '没有选择平台');
        setDeployName(null);
        refresh();
      
    });

  const importMcp = () =>
    runBusy('mcp', async () => {
      if (!mcpText.trim() || mcpTargets.size === 0) return;
      
        const r = await runCmd('hub_mcp_import', {
          text: mcpText,
          targets: [...mcpTargets],
        });

        if (r === undefined) return;
        toast(`写入 ${r.names.join('、')}\n${reportText(r.report.steps)}`);
        setMcpOpen(false);
        setMcpText('');
        refresh();
      
    });

  const removeMcp = (name: string, keys: string[]) =>
    runBusy(`mcp-remove:${name}`, async () => {
      
        const report = await runCmd('hub_mcp_remove', { name, targets: keys });

        if (report === undefined) return;
        toast(reportText(report.steps) || '先在右侧勾选平台');
        refresh();
      
    });

  // 把一条已存在的 MCP 条目写到更多平台（源条目由后端从所在平台读回）
  const deployMcp = () =>
    runBusy('mcp-deploy', async () => {
      const name = mcpDeployName!;
      
        const report = await runCmd('hub_mcp_deploy', {
          name,
          targets: [...mcpPicked],
        });

        if (report === undefined) return;
        toast(reportText(report.steps) || '没有选择平台');
        setMcpDeployName(null);
        refresh();
      
    });

  // 平台视图里的单平台移除：不进部署对话框，确认即删
  const removeSkillFrom = (name: string, key: string) =>
    runBusy(`remove:${name}:${key}`, async () => {
      
        const report = await runCmd('hub_skill_remove', { name, targets: [key] });

        if (report === undefined) return;
        toast(reportText(report.steps));
        refresh();
      
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
          className={`rounded-md px-3 py-1 text-sm transition-colors ${
            tab === 'all' ? 'bg-accent font-medium text-on-accent' : 'text-muted hover:bg-hover hover:text-fg'
          }`}
          onClick={() => setTab('all')}
        >
          总体
        </button>
        {snap.targets.map((t) => (
          <button
            key={t.key}
            className={`rounded-md px-3 py-1 text-sm transition-colors ${
              tab === t.key ? 'bg-accent font-medium text-on-accent' : 'text-muted hover:bg-hover hover:text-fg'
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

          <div className="flex items-center justify-between">
            <SubTabs
              value={pane}
              onChange={(v) => setPane(v as 'skills' | 'mcp')}
              items={[
                { key: 'skills', label: `技能库（${snap.skills.length}）` },
                { key: 'mcp', label: `MCP（${snap.mcp.length}）` },
              ]}
            />
            {pane === 'mcp' && (
              <Button variant="primary" size="sm" onClick={() => setMcpOpen(true)}>
                粘贴导入 MCP
              </Button>
            )}
          </div>

          <div className={pane === 'skills' ? 'rounded-lg border border-line bg-card p-3' : 'hidden'}>
            <div className="mb-2 flex items-center justify-between">
              <h2 className="text-sm font-semibold">
                技能库（{snap.skills.length}）
                <span className="ml-2 text-xs font-normal text-muted">移除请到对应平台页</span>
              </h2>
              <span className="flex items-center gap-3">
                {/* 图例：点阵颜色不单独表意（悬停有完整文字提示） */}
                <span className="flex items-center gap-3 text-xs text-muted">
                  <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-ok" />最新</span>
                  <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-warn" />待更新</span>
                  <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-line-strong" />未部署</span>
                </span>
                <Button size="sm" variant="ghost" onClick={() => setImportPick(true)}>
                  从平台导入
                </Button>
              </span>
            </div>
            <DataTable
              dense
              columns={[
                {
                  key: 'name',
                  header: '技能',
                  className: 'text-sm whitespace-nowrap',
                  cell: (skill) => (
                    <>
                      {skill.name}
                      {!skill.valid && <Tag tone="bad">{skill.reason}</Tag>}
                    </>
                  ),
                },
                {
                  key: 'desc',
                  header: '说明',
                  truncate: true,
                  className: 'max-w-[380px] text-xs text-muted',
                  title: (skill) => skill.description,
                  cell: (skill) => skill.description,
                },
                {
                  key: 'size',
                  header: '大小',
                  width: 80,
                  className: 'text-xs tabular-nums text-muted',
                  cell: (skill) => fmtSize(skill.size),
                },
                {
                  key: 'states',
                  header: (
                    <div className="grid w-44 grid-cols-8">
                      {snap.targets.map((t) => (
                        <span key={t.key} title={t.label} className="cursor-help text-center text-[9px] leading-4 text-muted">
                          {CODE[t.key] ?? t.key}
                        </span>
                      ))}
                    </div>
                  ),
                  width: 192,
                  cell: (skill) => (
                    <div className="grid w-44 grid-cols-8">
                      {snap.targets.map((t) => {
                        const state = skill.states.find(([k]) => k === t.key)?.[1] ?? 'missing';
                        return (
                          <span key={t.key} className="flex justify-center">
                            <button
                              className={`h-2.5 w-2.5 cursor-pointer rounded-full transition-transform hover:scale-125 ${
                                state === 'synced'
                                  ? 'bg-ok'
                                  : state === 'stale'
                                    ? 'bg-warn ring-2 ring-warn/30'
                                    : 'bg-line-strong'
                              }`}
                              title={`${t.label}：${STATE_LABEL[state] ?? state}`}
                              onClick={() => setTab(t.key)}
                            />
                          </span>
                        );
                      })}
                    </div>
                  ),
                },
                {
                  key: 'ops',
                  header: '操作',
                  width: 160,
                  cell: (skill) => (
                    <span className="flex gap-1">
                      <Button size="sm" onClick={() => { setDeployName(skill.name); setPicked(new Set()); }}>
                        部署
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => setImportPick(true)}>
                        从平台导入
                      </Button>
                    </span>
                  ),
                },
              ]}
              rows={snap.skills}
              rowKey={(skill) => skill.name}
              empty={<StateBlock kind="empty" title="技能库是空的" detail="把技能目录放进 data/skills/，或从平台导入。" />}
              rowClassName={() => 'group hover:bg-hover'}
            />
          </div>

          <div className={pane === 'mcp' ? 'rounded-lg border border-line bg-card p-3' : 'hidden'}>
            <h2 className="mb-2 text-sm font-semibold">
              MCP（{snap.mcp.length}）
              <span className="ml-2 text-xs font-normal text-muted">移除请到对应平台页</span>
            </h2>
            <DataTable
              dense
              columns={[
                {
                  key: 'name',
                  header: '名称',
                  className: 'text-sm',
                  cell: (row) => (
                    <>
                      {row.name}
                      {row.foreign && <Tag tone="muted">非本工具写入</Tag>}
                    </>
                  ),
                },
                { key: 'command', header: '命令', className: 'font-mono text-xs text-muted', cell: (row) => row.command },
                {
                  key: 'state',
                  header: '状态',
                  width: 96,
                  cell: (row) => <Tag tone={row.disabled ? 'warn' : 'ok'}>{row.disabled ? '停用' : '启用'}</Tag>,
                },
                {
                  key: 'targets',
                  header: '已写入平台',
                  className: 'text-xs text-muted',
                  cell: (row) => row.targets.join('、') || '—',
                },
                {
                  key: 'ops',
                  header: '操作',
                  width: 96,
                  cell: (row) => (
                    <span className="flex justify-end gap-1">
                      <Button
                        size="sm"
                        onClick={() => {
                          setMcpDeployName(row.name);
                          // 预选还没有这条的平台：已写入的不用再发
                          setMcpPicked(
                            new Set(
                              snap.targets.filter((t) => !row.present.includes(t.key)).map((t) => t.key),
                            ),
                          );
                        }}
                      >
                        部署
                      </Button>
                    </span>
                  ),
                },
              ]}
              rows={snap.mcp}
              rowKey={(row) => row.name}
              empty={<StateBlock kind="empty" title="还没有 MCP 条目" detail="点「粘贴导入 MCP」把配置写进选中的平台。" />}
              rowClassName={() => 'hover:bg-hover'}
            />
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

          <div className="flex items-center justify-between">
            <SubTabs
              value={pane}
              onChange={(v) => setPane(v as 'skills' | 'mcp')}
              items={[
                { key: 'skills', label: `技能（${activeTarget.installed.length}）` },
                { key: 'mcp', label: `MCP（${snap.mcp.filter((m) => m.present.includes(tab)).length}）` },
              ]}
            />
            {pane === 'mcp' && (
              <Button
                variant="primary"
                size="sm"
                onClick={() => {
                  setMcpTargets(new Set([tab]));
                  setMcpOpen(true);
                }}
              >
                粘贴导入到这个平台
              </Button>
            )}
          </div>

          {pane === 'skills' && (
          <div className="rounded-lg border border-line bg-card p-3">
            <h2 className="mb-2 text-sm font-semibold">
              {activeTarget.label} 的技能目录
              <span className="ml-2 text-xs font-normal text-muted">
                {activeTarget.installed.filter((s) => s.in_library).length} 个本工具分发 ·{' '}
                {activeTarget.installed.filter((s) => !s.in_library).length} 个平台已有
              </span>
            </h2>
            <DataTable
              dense
              columns={[
                {
                  key: 'name',
                  header: '技能',
                  className: 'text-sm',
                  cell: (inst) => {
                    const info = snap.skills.find((s) => s.name === inst.name);
                    return (
                      <>
                        {inst.name}
                        {info && !info.valid && <Tag tone="bad">{info.reason}</Tag>}
                        {info && (
                          <span className="block max-w-[420px] truncate text-xs text-muted" title={info.description}>
                            {info.description}
                          </span>
                        )}
                      </>
                    );
                  },
                },
                {
                  key: 'kind',
                  header: '分类',
                  width: 128,
                  cell: (inst) =>
                    inst.in_library ? (
                      <Tag tone="ok">本工具分发</Tag>
                    ) : (
                      <Tag tone="muted">平台已有（官方 / 手工放置）</Tag>
                    ),
                },
                {
                  key: 'sync',
                  header: '与库对账',
                  width: 96,
                  cell: (inst) =>
                    inst.in_library ? (
                      <Tag tone={inst.state === 'synced' ? 'ok' : 'warn'}>{STATE_LABEL[inst.state] ?? inst.state}</Tag>
                    ) : (
                      <span className="text-xs text-muted">—</span>
                    ),
                },
                {
                  key: 'ops',
                  header: '操作',
                  width: 192,
                  cell: (inst) => (
                    <span className="flex gap-1">
                      {inst.in_library && inst.state === 'stale' && (
                        <Button
                          size="sm"
                          onClick={() => {
                            setDeployName(inst.name);
                            setPicked(new Set([tab]));
                          }}
                        >
                          更新此副本
                        </Button>
                      )}
                      {inst.in_library && (
                        <Button
                          size="sm"
                          variant="danger"
                          loading={busy === `remove:${inst.name}:${tab}`}
                          disabled={!!busy}
                          onClick={async () => {
                            if (!(await confirmAction({ title: '确认操作', message: `从${activeTarget.label}移除「${inst.name}」？副本会先进回收站。`, danger: true }))) return;
                            void removeSkillFrom(inst.name, tab);
                          }}
                        >
                          移除
                        </Button>
                      )}
                      {!inst.in_library && (
                        <Button
                          size="sm"
                          loading={busy === `import:${inst.name}`}
                          disabled={!!busy}
                          onClick={() =>
                            runBusy(`import:${inst.name}`, async () => {
                              const res = await runCmd('hub_skill_import', { target: tab, name: inst.name });
                              if (res === undefined) return;
                              toast(`已导入技能库：${res.name}`);
                              refresh();
                            })
                          }
                        >
                          导入技能库
                        </Button>
                      )}
                    </span>
                  ),
                },
              ]}
              rows={activeTarget.installed}
              rowKey={(inst) => inst.name}
              empty={<StateBlock kind="empty" title="这个平台的技能目录是空的" detail="回「总体」页部署技能到这里，或把技能目录放进上面的路径。" />}
              rowClassName={() => 'hover:bg-hover'}
            />
          </div>
          )}

          {pane === 'mcp' && (
          <div className="rounded-lg border border-line bg-card p-3">
            <h2 className="mb-2 text-sm font-semibold">这个平台的 MCP（{snap.mcp.filter((m) => m.present.includes(tab)).length}）</h2>
            <DataTable
              dense
              columns={[
                { key: 'name', header: '名称', className: 'text-sm', cell: (row) => row.name },
                { key: 'command', header: '命令', className: 'font-mono text-xs text-muted', cell: (row) => row.command },
                {
                  key: 'state',
                  header: '状态',
                  width: 80,
                  cell: (row) => <Tag tone={row.disabled ? 'warn' : 'ok'}>{row.disabled ? '停用' : '启用'}</Tag>,
                },
                {
                  key: 'from',
                  header: '来源',
                  width: 128,
                  className: 'text-xs text-muted',
                  cell: (row) => (row.targets.includes(tab) ? '本工具部署' : '非本工具写入'),
                },
                {
                  key: 'ops',
                  header: '操作',
                  width: 80,
                  cell: (row) => (
                    <span className="flex justify-end gap-1">
                      <Button
                        size="sm"
                        onClick={() => {
                          setMcpDeployName(row.name);
                          setMcpPicked(
                            new Set(
                              snap.targets.filter((t) => !row.present.includes(t.key)).map((t) => t.key),
                            ),
                          );
                        }}
                      >
                        部署
                      </Button>
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
                    </span>
                  ),
                },
              ]}
              rows={snap.mcp.filter((m) => m.present.includes(tab))}
              rowKey={(row) => row.name}
              empty={<StateBlock kind="empty" title="这个平台还没有 MCP 条目" detail="回「总体」页点「粘贴导入 MCP」。" />}
              rowClassName={() => 'hover:bg-hover'}
            />
          </div>
          )}
        </>
      )}

      {/* 部署对话框：选平台 → 无冲突直接写；冲突时给「覆盖」确认（契约 16）。
          已部署（synced）的平台锁成「已勾选且不可改」，避免重复部署；
          移除不放在这里——到对应平台页的行内按钮做，部署与移除不再混在一起。 */}
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
          onAll={(keys) => setPicked(new Set(keys))}
          locked={
            new Set(
              (snap.skills.find((s) => s.name === deployName)?.states ?? [])
                .filter(([, st]) => st === 'synced')
                .map(([k]) => k),
            )
          }
          hints={Object.fromEntries(
            (snap.skills.find((s) => s.name === deployName)?.states ?? [])
              .filter(([, st]) => st === 'stale')
              .map(([k]) => [k, '待更新']),
          )}
        />
        <p className="mt-3 text-xs text-muted">
          已部署的平台已勾选且不可重复选择；部署会把库里的技能目录复制到其余平台，目标已有同名但内容不同的副本时报冲突。
        </p>

        {/* 覆盖是例外操作，收进分隔线下的次级区 */}
        <div className="mt-4 border-t border-line pt-3">
          <p className="mb-2 text-xs text-muted">目标已有同名但内容不同的副本时，可强制覆盖：</p>
          <Button
            size="sm"
            variant="danger"
            onClick={() => deploy(true)}
            disabled={picked.size === 0 || !!busy}
            loading={busy === 'deploy'}
          >
            强制覆盖部署
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
            onAll={(keys) => setMcpTargets(new Set(keys))}
          />
        </div>
      </Dialog>

      {/* MCP 行「部署」：把已有条目写到更多平台；源条目由后端从所在平台读回 */}
      <Dialog
        open={mcpDeployName !== null}
        title={`部署「${mcpDeployName ?? ''}」到其他平台`}
        onClose={() => setMcpDeployName(null)}
        footer={
          <>
            <Button onClick={() => setMcpDeployName(null)} disabled={!!busy}>
              取消
            </Button>
            <Button
              variant="primary"
              disabled={mcpPicked.size === 0 || !!busy}
              loading={busy === 'mcp-deploy'}
              onClick={deployMcp}
            >
              部署
            </Button>
          </>
        }
      >
        <TargetPicker
          targets={snap.targets}
          picked={mcpPicked}
          onToggle={(key) => toggle(mcpPicked, key, setMcpPicked)}
          onAll={(keys) => setMcpPicked(new Set(keys))}
          locked={new Set(snap.mcp.find((m) => m.name === mcpDeployName)?.present ?? [])}
          lockedLabel="已存在"
        />
        <p className="mt-3 text-xs text-muted">
          已存在的平台已勾选且不可改；条目内容从它当前所在的平台读回，按各平台格式写入，目标已有同名但内容不同的条目时报冲突，不会静默覆盖。
        </p>
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
