// 每日活跃：成长中心（活跃地图 + 连续天数 + 未完成任务），只读展示。
// **不代领任务奖励**——那属于用户自己的动作；这里只负责「今天算不算活跃」，
// 并把服务端的判定原样回报。
import { useQuery } from '@tanstack/react-query';
import { Tag } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { errMsg, queryCmd } from '@/lib/cmd';
import type { AccountView, DailyActive } from '@/lib/types';

export function DailyCard({ account }: { account: AccountView }) {
  const daily = useQuery<DailyActive>({
    queryKey: ['daily', account.iid],
    queryFn: () => queryCmd('activity_daily_status', { iid: account.iid }),
    retry: false,
    staleTime: 60_000,
  });

  const data = daily.data;

  return (
    <section className="border-t border-line px-3 py-2.5">
      <div className="mb-1.5 flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-semibold tracking-wide text-fg2">每日活跃</h3>
        {data && (
          <Tag tone={data.active ? 'ok' : 'warn'}>{data.active ? '今日已完成' : '今日未完成'}</Tag>
        )}
        <span className="flex-1" />
        <Button
          size="sm"
          variant="ghost"
          onClick={() => void daily.refetch()}
          loading={daily.isFetching}
        >
          刷新
        </Button>
      </div>

      {daily.isError && <p className="text-xs text-bad">读取失败：{errMsg(daily.error)}</p>}

      {data && (
        <div className="flex flex-col gap-1.5 text-xs">
          <div className="flex flex-wrap items-center gap-3 text-muted">
            <span>今日 score {data.score}</span>
            <span>连续 {data.streak_days} 天</span>
            {data.next_tier_remaining > 0 && <span>距下一档 {data.next_tier_remaining} 天</span>}
            <span>
              本月活跃 {data.month_active_days}/{data.month_total_days} 天
            </span>
          </div>
          <div className="text-fg2">{data.message}</div>
          {data.pending_tasks.length > 0 ? (
            <div className="flex flex-col gap-0.5">
              {data.pending_tasks.map((task) => (
                <div key={task.code} className="flex items-center gap-2 text-muted">
                  <span className="min-w-0 flex-1 truncate" title={task.code}>
                    {task.title || task.code}
                  </span>
                  <span className="tabular-nums">
                    {task.current}/{task.target}
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <div className="text-muted">成长任务已全部完成（共 {data.total_tasks} 项）。</div>
          )}
        </div>
      )}
    </section>
  );
}
