// 猫猫旅行卡片：只对 WorkBuddy 国内版渲染。
// 状态与地点来自平台接口；领取/派出是写操作，必须由用户点击或打开「自动旅行」后
// 由「猫猫旅行」定时任务执行。账号级偏好落 kv，重扫不会丢。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Checkbox, Select, StateBlock } from '@/components/ui/form';
import { Notice, Tag } from '@/components/ui';
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
    <section className="border-t border-line px-3 py-2.5">
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

      <div className="grid gap-x-6 gap-y-1 text-xs sm:grid-cols-2 lg:grid-cols-4">
        <div>
          <div className="text-muted">地点</div>
          <div className="truncate" title={locationName ?? undefined}>
            {locationName ?? '—'}
          </div>
        </div>
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
        <div>
          <div className="text-muted">自动旅行</div>
          <div>{prefs.enabled ? '已开启' : '已关闭'}</div>
        </div>
      </div>

      {status.letter && (
        <details className="mt-2 rounded-md border border-line bg-bg px-2 py-1.5 text-xs text-muted">
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
          <Button
            size="sm"
            onClick={depart}
            loading={busy === 'travel-depart'}
            disabled={!!busy || status.daily_limit_reached || locations.length === 0}
            title={locations.length === 0 ? '平台没有返回可去的地点' : undefined}
          >
            {state === 'arrived' ? '再次派出' : '派出去旅行'}
          </Button>
        )}
        {locations.length > 0 && (
          <Select
            value={selectedLocation === null ? '' : String(selectedLocation)}
            onChange={(e) => {
              const value = e.target.value ? Number(e.target.value) : null;
              setSelectedLocation(value);
              if (prefs.enabled) void savePrefs(true, value);
            }}
            disabled={!!busy}
            className="max-w-[220px]"
            title="选择旅行地点；开启自动旅行时这里就是默认地点"
          >
            {locations.map((location) => (
              <option key={location.id} value={location.id}>
                {location.name}
                {location.duration_hours_min && location.duration_hours_max
                  ? `（${location.duration_hours_min}-${location.duration_hours_max}h，+${location.reward_credit_min ?? '?'}-${location.reward_credit_max ?? '?'}）`
                  : ''}
              </option>
            ))}
          </Select>
        )}
        <label className="flex items-center gap-1.5 text-xs text-fg2">
          <Checkbox
            checked={prefs.enabled}
            disabled={!!busy || locations.length === 0}
            onChange={(e) => void savePrefs(e.target.checked, selectedLocation)}
          />
          参与自动旅行
        </label>
        <span className="text-xs text-muted">
          开启后，「猫猫旅行」定时任务会按这个地点领取并再派出。
        </span>
      </div>

      {records.length > 0 && (
        <div className="mt-2 border-t border-line/60 pt-1.5">
          <div className="mb-1 text-xs text-muted">最近记录</div>
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
            {records.slice(0, 5).map((record, index) => (
              <span key={record.record_id ?? index} className="tabular-nums">
                {record.travel_date ?? fmtTime(record.arrive_at)} · {record.location_name ?? '—'} ·{' '}
                {record.reward_credit === null ? '—' : `+${fmtAmount(record.reward_credit)}`}
              </span>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
