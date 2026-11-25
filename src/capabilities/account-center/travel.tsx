// 猫猫旅行卡片：只对 WorkBuddy 国内版渲染。
// 状态与地点来自平台接口；领取/派出是写操作，必须由用户点击或打开「自动旅行」后
// 由「猫猫旅行」定时任务执行。账号级偏好落 kv，重扫不会丢。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { Button, cn } from '@/components/ui/button';
import { StateBlock } from '@/components/ui/form';
import { Hint, Notice, Picker, Switch, Tag, Tooltip } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd, useBusy } from '@/lib/cmd';
import { fmtAmount, fmtTime } from '@/lib/format';
import { errText } from '@/lib/ipc';
import type { AccountView, ActivityOverview, TravelState } from '@/lib/types';

const STATE_META: Record<TravelState, { label: string; tone: 'ok' | 'warn' | 'bad' | 'muted' }> = {
  idle: { label: '待派出', tone: 'muted' },
  traveling: { label: '旅行中', tone: 'muted' },
  arrived: { label: '已回来', tone: 'ok' },
  limit: { label: '今日上限', tone: 'warn' },
  unknown: { label: '状态未知', tone: 'bad' },
};

export function TravelCard({ account }: { account: AccountView }) {
  const qc = useQueryClient();
  const { busy, run } = useBusy();
  const [selectedLocation, setSelectedLocation] = useState<number | null>(null);

  const overview = useQuery<ActivityOverview>({
    queryKey: ['activity', account.iid],
    queryFn: () => queryCmd('activity_overview', { iid: account.iid }),
    staleTime: 30_000,
  });

  useEffect(() => {
    if (!overview.data) return;
    setSelectedLocation(
      overview.data.prefs.location_id ??
        overview.data.config.locations[0]?.id ??
        null,
    );
  }, [overview.data]);

  if (overview.isLoading) {
    return (
      <section className="border-t border-line px-3 py-2.5 text-xs text-muted">
        正在读取猫猫旅行状态…
      </section>
    );
  }
  if (overview.isError) {
    return (
      <section className="border-t border-line">
        <StateBlock
          kind="error"
          title="读取猫猫旅行失败"
          detail={errText(overview.error)}
          onRetry={() => overview.refetch()}
        />
      </section>
    );
  }

  const data = overview.data;
  if (!data) return null;
  const status = data.status;
  const state: TravelState = status.state ?? 'unknown';
  const meta = STATE_META[state];
  const locations = data.config.locations;
  const prefs = data.prefs;
  const records = data.records;
  const events = data.events;
  const buddyName = status.buddy_name ?? data.config.buddy_name;
  const locationName = status.location?.name;
  const canDepart = state === 'idle' || state === 'arrived';
  const canClaim = state === 'arrived';

  const refreshAll = async () => {
    await Promise.all([
      overview.refetch(),
      qc.invalidateQueries({ queryKey: ['accounts'] }),
      qc.invalidateQueries({ queryKey: ['detail', account.iid] }),
    ]);
  };

  const claim = () =>
    run('travel-claim', async () => {
      const result = await runCmd('activity_claim', { iid: account.iid });
      if (result === undefined) return;
      toast(result.message);
      // 领取会加积分：顺手刷一次余额，列表不用等下一轮
      await queryCmd('checkin_credits', { iids: [account.iid] }).catch(() => {});
      await refreshAll();
    });

  const depart = () =>
    run('travel-depart', async () => {
      const result = await runCmd('activity_depart', {
        iid: account.iid,
        location: selectedLocation,
      });
      if (result === undefined) return;
      toast(result.message);
      await refreshAll();
    });

  const savePrefs = (enabled: boolean, location: number | null) =>
    run('travel-pref', async () => {
      const saved = await runCmd('activity_pref_set', {
        iid: account.iid,
        enabled,
        location,
      });
      if (saved === undefined) return;
      setSelectedLocation(location);
      toast(enabled ? '已开启自动旅行：定时任务会回来→领取→再派出' : '已关闭自动旅行');
      await Promise.all([
        qc.invalidateQueries({ queryKey: ['activity', account.iid] }),
        qc.invalidateQueries({ queryKey: ['schedules'] }),
      ]);
    });

  return (
    <section className="px-4 py-3">
      <div className="mb-1.5 flex flex-wrap items-center gap-2">
        <h3 className="text-xs font-semibold tracking-wide text-fg2">猫猫旅行</h3>
        {buddyName && <Tag tone="muted">{buddyName}</Tag>}
        <Tag tone={meta.tone}>{meta.label}</Tag>
        {status.daily_limit_reached && <Tag tone="warn">今日上限</Tag>}
        <span className="flex-1" />
        <Button
          size="sm"
          variant="ghost"
          onClick={() => overview.refetch()}
          disabled={!!busy}
        >
          刷新
        </Button>
      </div>

      {/* 事实行只在旅途中/回来后有意义：待派出时全是一排「—」，直接不画 */}
      {(state === 'traveling' || state === 'arrived') && (
        <div className="mb-2 flex flex-wrap gap-x-6 gap-y-1 text-xs">
          <div>
            <div className="text-muted">{state === 'traveling' ? '预计回来' : '回来时间'}</div>
            <div className="tabular-nums">{fmtTime(status.arrive_at)}</div>
          </div>
          <div>
            <div className="text-muted">本次奖励</div>
            <div className="tabular-nums">
              {status.reward_credit === null ? '—' : `+${fmtAmount(status.reward_credit)}`}
            </div>
          </div>
          {locationName && (
            <div>
              <div className="text-muted">地点</div>
              <Hint content={locationName} focusable={false} className="block truncate">
                {locationName}
              </Hint>
            </div>
          )}
        </div>
      )}

      {status.letter && (
        <details className="mt-2 rounded-md border border-line bg-sunken px-2 py-1.5 text-xs text-muted">
          <summary className="cursor-pointer">Buddy 的来信</summary>
          <p className="mt-1 whitespace-pre-wrap text-fg2">{status.letter}</p>
        </details>
      )}

      {data.warnings.length > 0 && (
        <div className="mt-2">
          <Notice tone="warn">{data.warnings.join('；')}</Notice>
        </div>
      )}

      <div className="mt-2 flex flex-wrap items-center gap-2">
        {canClaim && (
          <Button
            size="sm"
            variant="primary"
            onClick={claim}
            loading={busy === 'travel-claim'}
            disabled={!!busy || status.daily_limit_reached}
          >
            领取奖励
          </Button>
        )}
        {canDepart && (
          <Tooltip content={locations.length === 0 ? '平台没有返回可去的地点' : undefined}>
            <Button
              size="sm"
              onClick={depart}
              loading={busy === 'travel-depart'}
              disabled={!!busy || status.daily_limit_reached || locations.length === 0}
            >
              {state === 'arrived' ? '再次派出' : '派出去旅行'}
            </Button>
          </Tooltip>
        )}
        {locations.length > 0 && (
          <Hint content="选择旅行地点；开启自动旅行时这里就是默认地点" focusable={false}>
            <Picker
              label="旅行地点"
              value={selectedLocation === null ? '' : String(selectedLocation)}
              onValueChange={(next) => {
                const value = next ? Number(next) : null;
                setSelectedLocation(value);
                if (prefs.enabled) void savePrefs(true, value);
              }}
              disabled={!!busy}
              className="max-w-[220px]"
              options={locations.map((location) => ({
                value: String(location.id),
                label: `${location.name}${
                  location.duration_hours_min && location.duration_hours_max
                    ? `（${location.duration_hours_min}-${location.duration_hours_max}h，+${location.reward_credit_min ?? '?'}-${location.reward_credit_max ?? '?'}）`
                    : ''
                }`,
              }))}
            />
          </Hint>
        )}
        <label className="flex items-center gap-1.5 text-xs text-fg2">
          <Switch
            checked={prefs.enabled}
            onCheckedChange={(next) => void savePrefs(next, selectedLocation)}
            disabled={!!busy || locations.length === 0}
            label="参与自动旅行"
            hint="开启后，「猫猫旅行」定时任务会按这个地点领取并再派出"
            size="sm"
          />
          参与自动旅行
        </label>
      </div>

      {/* 旅行记录：本机事件为主、平台记录折叠；限高滚动，数据再多也不把卡片撑长 */}
      {(events.length > 0 || records.length > 0) && (
        <div className="mt-2 border-t border-line pt-1.5">
          <div className="mb-1 flex items-baseline gap-2 text-xs text-muted">
            <span>旅行记录</span>
            <span className="flex-1" />
            <span className="tabular-nums">本机 {events.length} 条 · 平台 {records.length} 条</span>
          </div>
          <div className="flex max-h-36 flex-col gap-0.5 overflow-y-auto text-xs">
            {events.map((event, index) => (
              <div key={`${event.kind}-${event.at}-${index}`} className="flex items-baseline gap-2">
                <span className="tabular-nums text-muted">{fmtTime(event.at)}</span>
                <span
                  className={cn(
                    'w-14 shrink-0 font-medium',
                    event.kind === 'claim' ? 'text-ok' : 'text-fg2',
                  )}
                >
                  {event.kind === 'depart' ? '派出' : event.kind === 'arrive' ? '回来' : '领到'}
                </span>
                <Hint
                  content={event.location}
                  focusable={false}
                  className="min-w-0 truncate text-fg2"
                >
                  {event.location || '—'}
                </Hint>
                {event.amount !== null && (
                  <span className="tabular-nums text-ok">+{fmtAmount(event.amount)}</span>
                )}
              </div>
            ))}
          </div>
          {records.length > 0 && (
            <details className="mt-1 text-xs text-muted">
              <summary className="cursor-pointer select-none">
                平台已完成旅行（{records.length}）
              </summary>
              <div className="mt-1 flex flex-col gap-0.5">
                {records.slice(0, 10).map((record, index) => (
                  <span key={record.record_id ?? index} className="tabular-nums">
                    {record.travel_date ?? fmtTime(record.arrive_at)} · {record.location_name ?? '—'} ·{' '}
                    {record.reward_credit === null ? '—' : `+${fmtAmount(record.reward_credit)}`}
                  </span>
                ))}
              </div>
            </details>
          )}
        </div>
      )}
    </section>
  );
}
