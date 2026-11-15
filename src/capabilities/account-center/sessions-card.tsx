// 账号详情里的「该账号的会话」区块：把会话页的账号维度接到账号中心。
// 只对有账号归属的来源有意义（WorkBuddy / CodeBuddy 的会话行带 uid）；
// Trae / Qoder / ZCode 的会话没有账号字段（显示「本机」），不下钻。
import { useQuery } from '@tanstack/react-query';
import { Button } from '@/components/ui/button';
import { DataTable, type Column } from '@/components/ui/table';
import { queryCmd } from '@/lib/cmd';
import { navigate } from '@/lib/nav';
import { fmtTime } from '@/lib/format';
import type { AccountView, Filter, SessionRow } from '@/lib/types';
import { STATE_LABELS } from '@/capabilities/sessions/grouping';

/** 会话行的账号列是客户端 uid；AccountView 只暴露 iid（`platform:account_id`）。 */
function uidOf(account: AccountView): string {
  const uid = account.iid.split(':')[1] ?? '';
  return uid.replace(/@intl$/, '');
}

const columns: Column<SessionRow>[] = [
  { key: 'title', header: '标题', truncate: true, cell: (row) => row.title || '（无标题）' },
  { key: 'source', header: '来源', width: 120, truncate: true, className: 'text-muted', cell: (row) => row.source_label },
  {
    key: 'time',
    header: '时间',
    width: 110,
    truncate: true,
    className: 'tabular-nums text-muted',
    cell: (row) => fmtTime(row.last_at),
  },
  {
    key: 'state',
    header: '状态',
    width: 72,
    truncate: true,
    cell: (row) => STATE_LABELS[row.state] ?? row.state,
  },
];

export function AccountSessionsCard({ account }: { account: AccountView }) {
  const uid = uidOf(account);
  const filter: Filter = {
    source: null, account: uid, state: null, q: null, origin: null,
    unnamed: false, pinned: false, hidden: false, body: null, tags: [], sort: 'last_at',
  };
  const sessions = useQuery({
    queryKey: ['account-sessions', uid],
    queryFn: () => queryCmd('session_page', { cursor: null, limit: 5, filter }),
    // 会话索引每 5 分钟自动重扫，面板开着跟着刷新即可
    refetchInterval: 60_000,
  });

  const total = sessions.data?.total ?? 0;
  const rows = sessions.data?.items ?? [];

  return (
    <section className="border-t border-line px-3 py-2.5">
      <div className="mb-1.5 flex items-baseline gap-2">
        <h3 className="text-xs font-semibold tracking-wide text-fg2">该账号的会话</h3>
        <span className="flex-1" />
        <span className="text-xs text-muted">共 {total} 条</span>
        {total > 0 && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => navigate('sessions', { account: uid })}
            title="去会话页，按这个账号筛选"
          >
            查看全部 →
          </Button>
        )}
      </div>
      <div className="max-h-52 overflow-auto">
        <DataTable
          columns={columns}
          rows={rows}
          rowKey={(row) => `${row.kind}|${row.source_key}|${row.account}|${row.session_id}`}
          loading={sessions.isLoading}
          dense
          stickyHead
          empty={<p className="py-3 text-xs text-muted">这个账号还没有会话记录。</p>}
        />
      </div>
    </section>
  );
}
