// 官方技能：各平台技能目录里存在、但还没进技能库的那些。
//
// 与技能库（个人技能）刻意分成两级：
//   · 技能库 = 本工具维护并分发的主体，默认展开，参与部署与同步；
//   · 官方技能 = 平台自带的，默认折叠，只读展示 + 单个「收录」。
// 后端分不清「官方预置」和「用户手工放进去」——磁盘上长得一样，所以文案不写成断言。
import { useMemo, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Tag } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import type { OfficialScan, OfficialSkill } from '@/lib/types';

export function OfficialSkills({
  count,
  target,
}: {
  /** 折叠状态下的数量：来自 hub_snapshot，不必为了显示一个数字先扫一遍目录 */
  count: number;
  /** 指定平台时只列这一家；不传 = 总体视图，按平台分组 */
  target?: string;
}) {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const [open, setOpen] = useState(false);

  // 展开才扫：日常视图里它只是一个折叠标题，不该为它读 8 个平台的技能目录
  const scan = useQuery<OfficialScan>({
    queryKey: ['official-scan'],
    queryFn: () => queryCmd('hub_official_scan'),
    enabled: open,
    retry: false,
  });

  const groups = useMemo(() => {
    const rows = (scan.data?.skills ?? []).filter((s) => !target || s.target === target);
    const byTarget = new Map<string, { target: string; label: string; skills: OfficialSkill[] }>();
    for (const row of rows) {
      const group = byTarget.get(row.target) ?? {
        target: row.target,
        label: row.label,
        skills: [],
      };
      group.skills.push(row);
      byTarget.set(row.target, group);
    }
    return [...byTarget.values()];
  }, [scan.data, target]);

  const take = (skill: OfficialSkill) =>
    run(`take:${skill.target}:${skill.name}`, async () => {
      const info = await runCmd('hub_skill_import', {
        target: skill.target,
        name: skill.name,
      });
      if (info === undefined) return;
      toast(`已收录到技能库：${info.name}`);
      // 收录后它从「官方」变成个人技能：两份数据都要重取，否则同一行会挂在两边
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['hub'] }),
        qc.invalidateQueries({ queryKey: ['official-scan'] }),
      ]);
    });

  // 0 个就没有「分类」可言：留一个空标题只会在日常视图里占一行噪音
  if (count === 0) return null;

  return (
    <div className="mt-3 border-t border-line pt-2">
      <button
        className="flex w-full cursor-pointer flex-wrap items-center gap-2 text-left"
        onClick={() => setOpen((value) => !value)}
        title="平台自带、还没进技能库的技能；展开后才读各平台目录"
      >
        <span className="text-xs text-muted">{open ? '▾' : '▸'}</span>
        <span className="text-sm font-semibold">官方技能</span>
        <Tag tone="muted">{count}</Tag>
        <span className="text-xs text-muted">平台自带 · 未入库 · 不参与统一分发</span>
        <span className="flex-1" />
        <span className="text-xs text-muted">{open ? '收起' : '展开查看'}</span>
      </button>

      {open && (
        <div className="mt-2">
          {scan.isLoading && (
            <p className="py-3 text-center text-xs text-muted">正在扫描各平台技能目录…</p>
          )}
          {scan.isError && (
            <StateBlock
              kind="error"
              title="扫描官方技能失败"
              detail={errText(scan.error)}
              onRetry={() => scan.refetch()}
            />
          )}
          {scan.data && groups.length === 0 && (
            <p className="py-3 text-center text-xs text-muted">
              各平台都没有未入库的技能。收录过的会变成上面的个人技能。
            </p>
          )}
          {groups.map((group) => (
            <section key={group.target} className="mb-2 last:mb-0">
              <div className="mb-1 flex flex-wrap items-baseline gap-2">
                <h4 className="text-xs font-semibold text-fg2">{group.label}</h4>
                <span className="text-xs text-muted">{group.skills.length} 个</span>
                <span className="flex-1" />
                <span className="text-xs text-muted">来源：平台技能目录</span>
              </div>
              <div className="rounded-md border border-line">
                {group.skills.map((skill) => {
                  const id = `take:${skill.target}:${skill.name}`;
                  return (
                    <div
                      key={skill.name}
                      className="flex items-center gap-2 border-b border-line/60 px-2 py-1.5 text-xs last:border-b-0"
                    >
                      <Tag tone="muted">官方</Tag>
                      <span className="shrink-0 font-medium">{skill.name}</span>
                      <span
                        className="min-w-0 flex-1 truncate text-muted"
                        title={skill.description}
                      >
                        {skill.description || '（没有说明）'}
                      </span>
                      <Button
                        size="sm"
                        variant="ghost"
                        loading={busy === id}
                        disabled={!!busy && busy !== id}
                        onClick={() => void take(skill)}
                        title="复制进技能库，之后就能像个人技能一样部署"
                      >
                        收录
                      </Button>
                    </div>
                  );
                })}
              </div>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
