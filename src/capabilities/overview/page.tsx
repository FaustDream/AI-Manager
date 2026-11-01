// 总览：统计卡片（账号数 / 总积分 / 会话数 / 技能数）。
// 不为总览加新接口（phase-6 §4），全部用已有命令在前端聚合。
import { useQuery } from '@tanstack/react-query';
import { call } from '@/lib/ipc';
import { fmtAmount } from '@/lib/format';
import { Card, Tag } from '@/components/ui';
import { SkeletonRows, StateBlock } from '@/components/ui/form';
import type { AccountView, HubSnapshot, Page, SessionRow } from '@/lib/types';

function Stat({ n, t }: { n: React.ReactNode; t: string }) {
  return (
    <Card className="min-w-[160px]">
      <div className="text-[28px] font-semibold tabular-nums">{n}</div>
      <div className="mt-0.5 text-xs text-muted">{t}</div>
    </Card>
  );
}

export default function OverviewPage() {
  const accounts = useQuery<AccountView[]>({
    queryKey: ['accounts'],
    queryFn: () => call<AccountView[]>('account_list'),
  });
  const sessions = useQuery<Page<SessionRow>>({
    queryKey: ['sessions', 'count'],
    queryFn: () => call<Page<SessionRow>>('session_page', { cursor: null, limit: 1, filter: {} }),
  });
  const hub = useQuery<HubSnapshot>({
    queryKey: ['hub'],
    queryFn: () => call<HubSnapshot>('hub_snapshot'),
  });

  if (accounts.isLoading || sessions.isLoading || hub.isLoading) {
    return <SkeletonRows rows={4} cols={2} />;
  }
  if (accounts.isError) {
    return <StateBlock kind="error" title="读账号失败" detail={String(accounts.error)} onRetry={() => accounts.refetch()} />;
  }

  const totalBalance = (accounts.data ?? [])
    .filter((a) => a.balance !== null)
    .reduce((sum, a) => sum + (a.balance ?? 0), 0);
  const online = (accounts.data ?? []).filter((a) => a.checked_in === true).length;

  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(180px,1fr))] gap-3">
        <Stat n={accounts.data?.length ?? 0} t="账号总数" />
        <Stat n={<span className="text-ok">{online}</span>} t="今日已签" />
        <Stat n={fmtAmount(totalBalance)} t="积分合计（本地已知的）" />
        <Stat n={sessions.data?.total ?? 0} t="本地会话数" />
        <Stat n={hub.data?.skills.length ?? 0} t="技能库" />
        <Stat n={hub.data?.targets.length ?? 0} t="目标平台" />
      </div>
      <Card>
        <h2 className="mb-2 text-sm font-semibold">平台</h2>
        <div className="flex flex-wrap gap-2">
          {(hub.data?.targets ?? []).map((t) => (
            <Tag key={t.key}>
              {t.label} · 技能 {t.skills} · MCP {t.mcp}
            </Tag>
          ))}
        </div>
      </Card>
    </div>
  );
}
