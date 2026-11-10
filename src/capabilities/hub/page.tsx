// 技能与 MCP 分发：分发中心（分发库 → 各软件）、软件分组（个人 + 自带）、手动导入。
//
// 两条硬规则：
//   · 「分发中心」是**分发入口**，不是全部技能的汇总——各软件自带的技能只在
//     对应软件标签页里成组展示，不做跨软件汇总列表；
//   · 个人技能与官方（自带 / 收录来的）技能不混在一个无标识的列表里：
//     每条都带来源标签，软件分组里还分成两个分区。
// 冲突不静默覆盖：deploy 需要 overwrite 确认，后端已兜底，前端给「覆盖」入口。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { SubTabs, SourceTag, TargetPicker } from './parts';
import { ImportDialog } from './import-dialog';
import { ManualSkillDialog } from './manual-import';
import { confirmAction } from '@/components/ui/confirm';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { useState, type ReactNode } from 'react';
import { errText } from '@/lib/ipc';
import { fmtSize } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { SkeletonRows, StateBlock } from '@/components/ui/form';
import { DataTable } from '@/components/ui/table';
import { Notice, Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import type { HubSnapshot, Step } from '@/lib/types';

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

const STATE_LABEL: Record<string, string> = { synced: '最新', stale: '待更新', missing: '未部署' };

/** 一个分区卡片：标题 + 一句话定位 + 右侧动作。分发中心与软件分组共用。 */
function PaneCard({
  title,
  hint,
  actions,
  children,
}: {
  title: ReactNode;
  hint?: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="rounded-lg border border-line bg-card p-3">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-sm font-semibold">
          {title}
          {hint && <span className="ml-2 text-xs font-normal text-muted">{hint}</span>}
        </h2>
        {actions && <span className="flex flex-wrap items-center gap-2">{actions}</span>}
      </div>
      <div className="overflow-x-auto">{children}</div>
    </div>
  );
}

/** 子标签（技能 / MCP）：与顶部主标签同款式，但更小一档。 */
export default function HubPage() {
  const qc = useQueryClient();
  const hub = useQuery<HubSnapshot>({
    queryKey: ['hub'],
    queryFn: () => queryCmd('hub_snapshot'),
  });

  const [deployName, setDeployName] = useState<string | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  // 视图标签：'distribute' = 分发中心（分发库 + 全部 MCP），其余为平台 key
  const [tab, setTab] = useState<string>('distribute');
  // 每个视图里再分技能 / MCP 两个子页
  const [pane, setPane] = useState<'skills' | 'mcp'>('skills');
  // 对话框动作（部署/移除/写 MCP）的加载态
  const { busy, run: runBusy } = useBusy();
  const [importPick, setImportPick] = useState(false);
  const [manualOpen, setManualOpen] = useState(false);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [mcpText, setMcpText] = useState('');
  const [mcpTargets, setMcpTargets] = useState<Set<string>>(new Set());
  // MCP 行「部署」对话框：把已有条目写到更多平台
  const [mcpDeployName, setMcpDeployName] = useState<string | null>(null);
  const [mcpPicked, setMcpPicked] = useState<Set<string>>(new Set());
  // 收录软件自带的 MCP：来源平台固定，其余平台由用户挑
  const [claim, setClaim] = useState<{ name: string; source: string } | null>(null);
  const [claimPicked, setClaimPicked] = useState<Set<string>>(new Set());

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

  // 收录一条软件自带的 MCP：后端在原平台补标记，再写到勾选的平台
  const claimMcp = () =>
    runBusy('mcp-claim', async () => {
      if (!claim) return;
      const report = await runCmd('hub_mcp_claim', {
        name: claim.name,
        source: claim.source,
        targets: [...claimPicked],
      });
      if (report === undefined) return;
      toast(reportText(report.steps) || '已收录进分发库');
      setClaim(null);
      refresh();
    });

  // 软件分组里的单平台移除：不进部署对话框，确认即删
  const removeSkillFrom = (name: string, key: string) =>
    runBusy(`remove:${name}:${key}`, async () => {
      const report = await runCmd('hub_skill_remove', { name, targets: [key] });

      if (report === undefined) return;
      toast(reportText(report.steps));
      refresh();
    });

  // 收录软件自带的一条技能进分发库（后端会记下来源）
  const takeSkill = (name: string, key: string) =>
    runBusy(`take:${name}`, async () => {
      const info = await runCmd('hub_skill_import', { target: key, name });
      if (info === undefined) return;
      toast(`已收录进分发库：${info.name}`);
      refresh();
    });

  // 一个软件分组里的自带技能一次收完
  const takeAllSkills = (names: string[], key: string) =>
    runBusy('take-all', async () => {
      if (names.length === 0) return;
      const report = await runCmd('hub_skill_import_many', { target: key, names });
      if (report === undefined) return;
      toast(
        report.errors.length
          ? `收录 ${report.imported.length} 个，${report.errors.length} 个失败：${report.errors[0]}`
          : `已收录进分发库：${report.imported.join('、')}`,
      );
      refresh();
    });

  // 骨架行必须待在 <tbody> 里：直接返回 <tr> 会被浏览器搬出表格，控制台报 hydration 错
  if (hub.isLoading) {
    return (
      <table className="w-full border-collapse">
        <tbody>
          <SkeletonRows rows={4} cols={3} />
        </tbody>
      </table>
    );
  }
  if (hub.isError) {
    return (
      <StateBlock
        kind="error"
        title="读技能库失败"
        detail={errText(hub.error)}
        onRetry={() => hub.refetch()}
      />
    );
  }

  const snap = hub.data!;
  const labelOf = (key: string) => snap.targets.find((t) => t.key === key)?.label ?? key;
  const conflicts = snap.skills.filter((s) => s.states.some(([, state]) => state === 'stale'));
  const activeTarget = snap.targets.find((t) => t.key === tab);
  // 分发库的 MCP：只列本工具写过的条目（自带条目在各软件分组里，不进汇总）
  const mcpLibrary = snap.mcp.filter((m) => m.targets.length > 0);
  // 分发库里的技能按名字索引：软件分组的个人技能要显示它的来源
  const libraryByName = new Map(snap.skills.map((s) => [s.name, s]));

  const installed = activeTarget?.installed ?? [];
  const mine = installed.filter((s) => s.in_library);
  const builtin = installed.filter((s) => !s.in_library);
  const platformMcp = activeTarget ? snap.mcp.filter((m) => m.present.includes(tab)) : [];
  const myMcp = platformMcp.filter((m) => m.targets.includes(tab));
  const builtinMcp = platformMcp.filter((m) => !m.targets.includes(tab));

  const importButtons = (key?: string) => (
    <>
      <Button variant="primary" size="sm" onClick={() => setManualOpen(true)}>
        手动导入技能
      </Button>
      <Button size="sm" onClick={() => setImportPick(true)}>
        从软件导入
      </Button>
      {key && (
        <span className="text-xs text-muted">导入后即可部署到任意软件</span>
      )}
    </>
  );

  return (
    <div className="flex flex-col gap-3">
      {/* 分发中心 = 分发库 + 本工具写过的 MCP；单个软件 = 该软件的技能副本与 MCP 配置 */}
      <div className="flex flex-wrap items-center gap-1 border-b border-line pb-2">
        <button
          className={`rounded-md px-3 py-1 text-sm transition-colors ${
            tab === 'distribute'
              ? 'bg-accent font-medium text-on-accent'
              : 'text-muted hover:bg-hover hover:text-fg'
          }`}
          onClick={() => setTab('distribute')}
          title="技能与 MCP 的分发入口：把分发库部署到各软件"
        >
          分发中心
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

      {tab === 'distribute' && (
        <>
          <Notice>
            这里是<strong>分发中心</strong>：把分发库里的技能与 MCP 部署到各个软件。
            它不是全部技能的汇总——各软件自带的技能请到对应软件标签页里看。
          </Notice>

          {conflicts.length > 0 && (
            <Notice tone="warn">
              {conflicts.length} 个技能的平台副本与库不一致（启动自动同步会在下次启动时刷新已部署的平台）。
            </Notice>
          )}

          <div className="flex flex-wrap items-center justify-between gap-2">
            <SubTabs
              value={pane}
              onChange={(v) => setPane(v as 'skills' | 'mcp')}
              items={[
                { key: 'skills', label: `技能分发库（${snap.skills.length}）` },
                { key: 'mcp', label: `MCP 分发库（${mcpLibrary.length}）` },
              ]}
            />
            <span className="flex flex-wrap items-center gap-2">
              {pane === 'skills' ? (
                importButtons()
              ) : (
                <>
                  <Button
                    variant="primary"
                    size="sm"
                    onClick={() => {
                      setMcpTargets(new Set());
                      setMcpOpen(true);
                    }}
                  >
                    手动导入 MCP
                  </Button>
                  <span className="text-xs text-muted">写入选中软件后即进入分发库</span>
                </>
              )}
            </span>
          </div>

          {pane === 'skills' && (
            <PaneCard
              title={`技能分发库（${snap.skills.length}）`}
              hint="个人技能与收录来的官方技能 · 本工具维护并分发"
              actions={
                <span className="flex items-center gap-3 text-xs text-muted">
                  <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-ok" />最新</span>
                  <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-warn" />待更新</span>
                  <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-full bg-line-strong" />未部署</span>
                </span>
              }
            >
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
                    key: 'origin',
                    header: '来源',
                    width: 128,
                    cell: (skill) => (
                      <SourceTag
                        kind={skill.origin === 'official' ? 'official' : 'manual'}
                        from={skill.origin_target}
                        label={labelOf}
                      />
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
                    width: 96,
                    cell: (skill) => (
                      <Button size="sm" onClick={() => { setDeployName(skill.name); setPicked(new Set()); }}>
                        部署
                      </Button>
                    ),
                  },
                ]}
                rows={snap.skills}
                rowKey={(skill) => skill.name}
                empty={
                  <StateBlock
                    kind="empty"
                    title="分发库是空的"
                    detail="点「手动导入技能」把自己准备的技能放进来，或「从软件导入」从各软件里收。"
                  />
                }
                rowClassName={() => 'group hover:bg-hover'}
              />
            </PaneCard>
          )}

          {pane === 'mcp' && (
            <PaneCard
              title={`MCP 分发库（${mcpLibrary.length}）`}
              hint="本工具写入的条目 · 可分发到各软件"
            >
              <DataTable
                dense
                columns={[
                  { key: 'name', header: '名称', className: 'text-sm', cell: (row) => row.name },
                  { key: 'command', header: '命令', className: 'font-mono text-xs text-muted', cell: (row) => row.command },
                  {
                    key: 'state',
                    header: '状态',
                    width: 96,
                    cell: (row) => <Tag tone={row.disabled ? 'warn' : 'ok'}>{row.disabled ? '停用' : '启用'}</Tag>,
                  },
                  {
                    key: 'targets',
                    header: '已分发到',
                    className: 'text-xs text-muted',
                    cell: (row) => row.targets.map(labelOf).join('、') || '—',
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
                rows={mcpLibrary}
                rowKey={(row) => row.name}
                empty={
                  <StateBlock
                    kind="empty"
                    title="分发库里还没有 MCP 条目"
                    detail="点「手动导入 MCP」粘贴一份配置，或在某个软件的自带条目里收录。"
                  />
                }
                rowClassName={() => 'hover:bg-hover'}
              />
            </PaneCard>
          )}
        </>
      )}

      {activeTarget && (
        <>
          <div className="rounded-lg border border-line bg-card p-3 text-sm">
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
              <span className="font-semibold">{activeTarget.label}</span>
              <span className="text-xs text-muted">
                技能目录 {activeTarget.skills} 个（个人 {mine.length} · 自带 {builtin.length}）· MCP{' '}
                {activeTarget.mcp} 条
              </span>
            </div>
            <div className="mt-1 truncate text-xs text-muted" title={activeTarget.skill_dir}>
              技能目录：{activeTarget.skill_dir}
            </div>
            <div className="truncate text-xs text-muted" title={activeTarget.mcp_file}>
              MCP 配置：{activeTarget.mcp_file}
            </div>
          </div>

          <div className="flex flex-wrap items-center justify-between gap-2">
            <SubTabs
              value={pane}
              onChange={(v) => setPane(v as 'skills' | 'mcp')}
              items={[
                { key: 'skills', label: `技能（个人 ${mine.length} · 自带 ${builtin.length}）` },
                { key: 'mcp', label: `MCP（个人 ${myMcp.length} · 自带 ${builtinMcp.length}）` },
              ]}
            />
            <span className="flex flex-wrap items-center gap-2">
              {pane === 'skills' ? (
                importButtons(tab)
              ) : (
                <>
                  <Button
                    variant="primary"
                    size="sm"
                    onClick={() => {
                      setMcpTargets(new Set([tab]));
                      setMcpOpen(true);
                    }}
                  >
                    手动导入 MCP
                  </Button>
                  <span className="text-xs text-muted">写进{activeTarget.label}后即进入分发库</span>
                </>
              )}
            </span>
          </div>

          {pane === 'skills' && (
            <>
              <PaneCard
                title={`${activeTarget.label} 的个人技能（${mine.length}）`}
                hint="分发库里的技能在这个软件的副本"
              >
                <DataTable
                  dense
                  columns={[
                    {
                      key: 'name',
                      header: '技能',
                      className: 'text-sm',
                      cell: (inst) => {
                        const info = libraryByName.get(inst.name);
                        return (
                          <>
                            {inst.name}
                            {info && !info.valid && <Tag tone="bad">{info.reason}</Tag>}
                            {inst.description && (
                              <span className="block max-w-[420px] truncate text-xs text-muted" title={inst.description}>
                                {inst.description}
                              </span>
                            )}
                          </>
                        );
                      },
                    },
                    {
                      key: 'origin',
                      header: '来源',
                      width: 128,
                      cell: (inst) => {
                        const info = libraryByName.get(inst.name);
                        return (
                          <SourceTag
                            kind={info?.origin === 'official' ? 'official' : 'manual'}
                            from={info?.origin_target}
                            label={labelOf}
                          />
                        );
                      },
                    },
                    {
                      key: 'sync',
                      header: '与库对账',
                      width: 96,
                      cell: (inst) => (
                        <Tag tone={inst.state === 'synced' ? 'ok' : 'warn'}>
                          {STATE_LABEL[inst.state] ?? inst.state}
                        </Tag>
                      ),
                    },
                    {
                      key: 'ops',
                      header: '操作',
                      width: 192,
                      cell: (inst) => (
                        <span className="flex gap-1">
                          {inst.state === 'stale' && (
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
                        </span>
                      ),
                    },
                  ]}
                  rows={mine}
                  rowKey={(inst) => inst.name}
                  empty={
                    <StateBlock
                      kind="empty"
                      title="这个软件还没有分发来的技能"
                      detail="回「分发中心」把技能部署过来；软件自带的技能在下方分区里。"
                    />
                  }
                  rowClassName={() => 'hover:bg-hover'}
                />
              </PaneCard>

              <PaneCard
                title={`${activeTarget.label} 自带技能（${builtin.length}）`}
                hint="软件预置或你自己放进来的，不在分发库里"
                actions={
                  <Button
                    size="sm"
                    loading={busy === 'take-all'}
                    disabled={builtin.length === 0 || !!busy}
                    onClick={() => void takeAllSkills(builtin.map((s) => s.name), tab)}
                  >
                    全部收录
                  </Button>
                }
              >
                <DataTable
                  dense
                  columns={[
                    {
                      key: 'name',
                      header: '技能',
                      className: 'text-sm',
                      cell: (inst) => (
                        <>
                          {inst.name}
                          {inst.description && (
                            <span className="block max-w-[420px] truncate text-xs text-muted" title={inst.description}>
                              {inst.description}
                            </span>
                          )}
                        </>
                      ),
                    },
                    { key: 'origin', header: '来源', width: 96, cell: () => <SourceTag kind="builtin" /> },
                    {
                      key: 'ops',
                      header: '操作',
                      width: 112,
                      cell: (inst) => (
                        <Button
                          size="sm"
                          variant="ghost"
                          loading={busy === `take:${inst.name}`}
                          disabled={!!busy && busy !== `take:${inst.name}`}
                          onClick={() => void takeSkill(inst.name, tab)}
                          title="复制进分发库，之后就能像个人技能一样部署到其他软件"
                        >
                          收录
                        </Button>
                      ),
                    },
                  ]}
                  rows={builtin}
                  rowKey={(inst) => inst.name}
                  empty={
                    <StateBlock
                      kind="empty"
                      title="没有自带技能"
                      detail="这个软件的技能目录里没有库里之外的东西。"
                    />
                  }
                  rowClassName={() => 'hover:bg-hover'}
                />
              </PaneCard>
            </>
          )}

          {pane === 'mcp' && (
            <>
              <PaneCard
                title={`${activeTarget.label} 的个人条目（${myMcp.length}）`}
                hint="本工具写入，可在分发中心再分发"
              >
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
                      key: 'ops',
                      header: '操作',
                      width: 144,
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
                          <Button
                            size="sm"
                            variant="danger"
                            onClick={() => removeMcp(row.name, [tab])}
                            loading={busy === `mcp-remove:${row.name}`}
                            disabled={!!busy}
                          >
                            移除
                          </Button>
                        </span>
                      ),
                    },
                  ]}
                  rows={myMcp}
                  rowKey={(row) => row.name}
                  empty={
                    <StateBlock
                      kind="empty"
                      title="这个软件还没有分发来的 MCP"
                      detail="回「分发中心」部署，或点上方「手动导入 MCP」写一条进来。"
                    />
                  }
                  rowClassName={() => 'hover:bg-hover'}
                />
              </PaneCard>

              <PaneCard
                title={`${activeTarget.label} 自带条目（${builtinMcp.length}）`}
                hint="软件自己配置的，不在分发库里"
              >
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
                          <span className="ml-2 inline-block align-middle">
                            <SourceTag kind="builtin" />
                          </span>
                        </>
                      ),
                    },
                    { key: 'command', header: '命令', className: 'font-mono text-xs text-muted', cell: (row) => row.command },
                    {
                      key: 'state',
                      header: '状态',
                      width: 80,
                      cell: (row) => <Tag tone={row.disabled ? 'warn' : 'ok'}>{row.disabled ? '停用' : '启用'}</Tag>,
                    },
                    {
                      key: 'ops',
                      header: '操作',
                      width: 112,
                      cell: (row) => (
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            setClaim({ name: row.name, source: tab });
                            setClaimPicked(
                              new Set(snap.targets.filter((t) => t.key !== tab && !row.present.includes(t.key)).map((t) => t.key)),
                            );
                          }}
                          title="收进分发库，之后就能部署到其他软件"
                        >
                          收录
                        </Button>
                      ),
                    },
                  ]}
                  rows={builtinMcp}
                  rowKey={(row) => row.name}
                  empty={
                    <StateBlock
                      kind="empty"
                      title="没有自带 MCP 条目"
                      detail="这个软件的配置里都是本工具写入的条目。"
                    />
                  }
                  rowClassName={() => 'hover:bg-hover'}
                />
              </PaneCard>
            </>
          )}
        </>
      )}

      {/* 部署对话框：选平台 → 无冲突直接写；冲突时给「覆盖」确认（契约 16）。
          已部署（synced）的平台锁成「已勾选且不可改」，避免重复部署；
          移除不放在这里——到对应软件页的行内按钮做，部署与移除不再混在一起。 */}
      <Dialog
        open={deployName !== null}
        title={`部署「${deployName ?? ''}」到软件`}
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
          已部署的软件已勾选且不可重复选择；部署会把分发库里的技能目录复制到其余软件，目标已有同名但内容不同的副本时报冲突。
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
      <ManualSkillDialog open={manualOpen} onClose={() => setManualOpen(false)} />

      <Dialog
        open={mcpOpen}
        title="手动导入 MCP"
        subtitle="粘贴一份 MCP 配置，写入选中的软件；写进去的条目会进分发库，可再分发到其他软件"
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
              写入选中软件
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
        title={`部署「${mcpDeployName ?? ''}」到其他软件`}
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
          已存在的软件已勾选且不可改；条目内容从它当前所在的软件读回，按各软件格式写入，目标已有同名但内容不同的条目时报冲突，不会静默覆盖。
        </p>
      </Dialog>

      {/* 收录软件自带的 MCP：来源软件固定（原位打标记），其余软件由用户挑 */}
      <Dialog
        open={claim !== null}
        title={`收录「${claim?.name ?? ''}」到分发库`}
        subtitle="收录后它算本工具管理的条目，可以从分发中心再部署到其他软件"
        onClose={() => setClaim(null)}
        footer={
          <>
            <Button onClick={() => setClaim(null)} disabled={!!busy}>
              取消
            </Button>
            <Button
              variant="primary"
              disabled={!!busy}
              loading={busy === 'mcp-claim'}
              onClick={claimMcp}
            >
              收录
            </Button>
          </>
        }
      >
        <p className="mb-2 text-xs text-muted">
          来源软件：{claim ? labelOf(claim.source) : ''}（只补归属标记，配置内容原样保留）。勾选要同时写入的其他软件：
        </p>
        <TargetPicker
          targets={snap.targets}
          picked={claimPicked}
          onToggle={(key) => toggle(claimPicked, key, setClaimPicked)}
          onAll={(keys) => setClaimPicked(new Set(keys))}
          locked={claim ? new Set([claim.source]) : undefined}
          lockedLabel="来源"
        />
      </Dialog>
    </div>
  );
}
