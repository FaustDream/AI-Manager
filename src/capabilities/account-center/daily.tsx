// 每日活跃：成长中心（活跃地图 + 连续天数 + 未完成任务）。
//
// 补活跃只做一件事：用内置免费模型发一条「你好」。**不代领任务奖励**——那属于
// 用户自己的动作；这里只负责「今天算不算活跃」，并把服务端的判定原样回报。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { Tag } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { errMsg, queryCmd, runCmd, useBusy } from '@/lib/cmd';
import type { AccountView, DailyActive } from '@/lib/types';

export function DailyCard({ account }: { account: AccountView }) {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  // 自动补活跃：签到成功后自动发一条。默认关——它会在后台真的发消息。
  const [auto, setAuto] = useState(false);

  const daily = useQuery<DailyActive>({
    queryKey: ['daily', account.iid],
    queryFn: () => queryCmd('activity_daily_status', { iid: account.iid }),
    retry: false,
    staleTime: 60_000,
  });

  useEffect(() => {
    queryCmd('settings_get', { key: 'activity.daily_hello.enabled' })
      .then((v) => setAuto(v === true))
      .catch(() => {});
  }, []);

  const data = daily.data;

  const hello = () =>
    run('hello', async () => {
      const next = await runCmd('activity_daily_hello', { iid: account.iid });
      if (next === undefined) return;
      // 后端已经把服务端判定结果组装好了：直接用，不在这里二次推断
      qc.setQueryData(['daily', account.iid], next);
      toast(next.message);
    });

  return (
    <section className="border-t border-line px-3 py-2.5">
      <div className="mb-1.5 flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-semibold tracking-wide text-fg2">每日活跃</h3>
        {data && (
          <Tag tone={data.active ? 'ok' : 'warn'}>{data.active ? '今日已完成' : '今日未完成'}</Tag>
        )}
        <span className="flex-1" />
        <label
          className="flex items-center gap-1.5 text-xs text-muted"
          title="签到成功后自动发一条「你好」补活跃；同一天只发一次"
        >
          <Checkbox
            checked={auto}
            onChange={(e) => {
              const on = e.target.checked;
              setAuto(on);
              void queryCmd('settings_set', { key: 'activity.daily_hello.enabled', value: on })
                .then(() =>
                  toast(on ? '已开启：签到后自动补活跃' : '已关闭自动补活跃'),
                )
                .catch((err) => {
                  setAuto(!on);
                  toast(errMsg(err));
                });
            }}
          />
          自动
        </label>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => void daily.refetch()}
          loading={daily.isFetching}
        >
          刷新
        </Button>
        <Button
          size="sm"
          onClick={hello}
          loading={busy === 'hello'}
          disabled={!!busy || !data || data.active || data.hello_sent}
          title={
            data?.hello_sent
              ? '今天已经补过一次，不重复发送'
              : '用内置免费模型发一条「你好」，再回读活跃地图确认'
          }
        >
          {data?.hello_sent ? '今日已补发' : '补发「你好」'}
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
