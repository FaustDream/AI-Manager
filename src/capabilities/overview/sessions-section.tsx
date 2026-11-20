// 仪表盘的「会话」区块：会话维度的 KPI 瓦片 + 排名列表。
// 口径（与后端 session_stats 一致）：`active` 与两个排名是区间口径（区间内有活动、
// 不含已归档）；`total` / `archived` / `unnamed` 是存量口径，换区间不变。
// 遵循仪表盘既定口味：瓦片只留数字、解释进 tooltip；排名列表 + 行底 3px 份额条。
import { useQuery } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { StateBlock } from '@/components/ui/form';
import { queryCmd } from '@/lib/cmd';
import { errText } from '@/lib/ipc';
import { navigate } from '@/lib/nav';
import type { SessionStats } from '@/lib/types';
import { Metric, SectionTitle, ShareBar, shareText } from './metric';
import { RangeControl, useRange } from './range';

export function SessionsSection() {
  // 区间独立一份：会话索引、Token 采集、积分快照互不相干
  const range = useRange(7);
  const stats = useQuery<SessionStats>({
    queryKey: ['session-stats', range.from, range.to],
    queryFn: () => queryCmd('session_stats', { from: range.from, to: range.to }),
    // 统计只读本地索引表，代价低；跟仪表盘其他区块同节奏自动刷新
    refetchInterval: 5 * 60_000,
  });

  const data = stats.data;

  return (
    <section className="rounded-lg border border-line bg-card shadow-card p-3">
      <SectionTitle
        className="mb-0"
        right={
          <span className="flex flex-wrap items-center justify-end gap-2">
            <RangeControl range={range} />
            <Button size="sm" variant="ghost" onClick={() => navigate('sessions')}>
              去会话页 →
            </Button>
          </span>
        }
      >
        会话（全部 AI 客户端）
      </SectionTitle>

      {stats.isLoading && (
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          {Array.from({ length: 4 }).map((_, index) => (
            <div key={index} className="h-[92px] skeleton-block rounded-lg bg-hover" />
          ))}
        </div>
      )}

      {stats.isError && (
        <StateBlock
          kind="error"
          title="读会话统计失败"
          detail={errText(stats.error)}
          onRetry={() => stats.refetch()}
        />
      )}

      {data && (
        <div className="flex flex-col gap-3">
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
            <Metric
              label={`活跃 · ${range.label}`}
              value={String(data.active)}
              hint="区间内有活动的会话数（不含已归档）"
            />
            <Metric
              label="会话总数"
              value={String(data.total)}
              hint={`其中已归档 ${data.archived} 条（归档也是资产，计入总数）`}
            />
            <Metric
              label="已归档"
              value={String(data.archived)}
              hint="已归档但仍在索引里的会话，会话页可撤销归档"
            />
            <Metric
              label="未命名"
              value={String(data.unnamed)}
              hint="标题还是占位文案的会话（不含已归档），会话页有「未命名」快筛"
            />
          </div>

          <div className="grid gap-3 lg:grid-cols-2">
            <div className="rounded-lg border border-line/70 p-3">
              {/* 表头与排名行同一内边距（px-3）：差 8px 整列数字就会看着歪 */}
              <div className="mb-1 flex items-baseline justify-between gap-2 px-3 text-xs text-muted">
                <span>来源</span>
                <span>会话数 · 占比</span>
              </div>
              <RankedSources bySource={data.by_source} total={data.active} />
            </div>

            <div className="rounded-lg border border-line/70 p-3">
              <div className="mb-1 flex items-baseline justify-between gap-2 px-3 text-xs text-muted">
                <span>项目 Top 8</span>
                <span>会话数 · 占比</span>
              </div>
              {data.by_project.length === 0 ? (
                <p className="px-1 py-3 text-xs text-muted">还没有项目信息（会话都来自临时工作区或无项目路径）。</p>
              ) : (
                <RankedProjects byProject={data.by_project} total={data.active} />
              )}
            </div>
          </div>

        </div>
      )}
    </section>
  );
}

/** 来源排名：排名列表 + 行底 3px 份额条，点击落到对应来源筛选的会话页。 */
function RankedSources({
  bySource,
  total,
}: {
  bySource: SessionStats['by_source'];
  total: number;
}) {
  const max = Math.max(...bySource.map((row) => row.count), 1);
  return (
    <div>
      {bySource.map((row) => (
        <button
          key={row.source_key}
          title={`${row.label} · ${row.count} 条会话（点击去会话页按来源筛选）`}
          className="group relative flex h-11 w-full items-center gap-3 rounded-[10px] px-3 pb-1.5 text-left text-sm text-fg hover:bg-hover/60"
          onClick={() => navigate('sessions', { source: row.source_key })}
        >
          <ShareBar pct={(row.count / max) * 100} />
          <span className="relative min-w-0 flex-1 truncate">{row.label}</span>
          <span className="relative tabular-nums">{row.count}</span>
          <span className="relative w-10 text-right text-xs tabular-nums text-muted">
            {shareText(total === 0 ? 0 : (row.count / total) * 100)}
          </span>
        </button>
      ))}
    </div>
  );
}

/** 项目排名：同样的排名列表；点击按项目路径筛选（会话页 q 匹配 cwd）。 */
function RankedProjects({
  byProject,
  total,
}: {
  byProject: SessionStats['by_project'];
  total: number;
}) {
  const max = Math.max(...byProject.map((row) => row.count), 1);
  return (
    <div>
      {byProject.map((row) => (
        <div
          key={row.project}
          title={`${row.project} · ${row.count} 条会话`}
          className="group relative flex h-11 w-full items-center gap-3 rounded-[10px] px-3 pb-1.5 text-left text-sm text-fg hover:bg-hover/60"
        >
          <ShareBar pct={(row.count / max) * 100} />
          <span className="relative min-w-0 flex-1 truncate" title={row.project}>
            {row.project}
          </span>
          <span className="relative tabular-nums">{row.count}</span>
          <span className="relative w-10 text-right text-xs tabular-nums text-muted">
            {shareText(total === 0 ? 0 : (row.count / total) * 100)}
          </span>
        </div>
      ))}
    </div>
  );
}
