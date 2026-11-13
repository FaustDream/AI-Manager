// 全局 Provider：QueryClient（服务端状态）+ 事件桥（§6.3 的前端侧）。
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { listen } from '@/lib/events';
import { useQueryClient } from '@tanstack/react-query';
import { toast } from '@/components/ui/toast';
import { queryCmd } from '@/lib/cmd';
import type { LoginOutcome, LogLine, RoundResult, SessionScanReport } from '@/lib/types';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: 1, staleTime: 15_000, refetchOnWindowFocus: false },
  },
});

/** 事件桥：后端推 log:line / session:scan / checkin:round / credits:round /
 *  claim:recorded / activity:round / zcode:claim / account:login，
 *  前端在这里统一接住，按能力失效对应查询——页面不用自己 listen。 */
function EventBridges() {
  const qc = useQueryClient();
  // 后台扫描每 5 分钟跑一次：同一个来源错误只提示一次，避免弹窗刷屏；
  // 错误消失后清空，下一次再坏还能再提示。
  const lastScanError = useRef<string | null>(null);
  useEffect(() => {
    const unbinds = [
      listen<RoundResult>('checkin:round', (e) => {
        qc.setQueryData(['daily'], e.payload);
        qc.invalidateQueries({ queryKey: ['accounts'] });
        // 签到会写领取记录与批次：详情面板开着时也要跟着换，否则要等用户手动刷新
        qc.invalidateQueries({ queryKey: ['detail'] });
        toast(`每日轮询：${e.payload.message}`);
      }),
      listen<SessionScanReport>('session:scan', (e) => {
        qc.invalidateQueries({ queryKey: ['sessions'] });
        qc.invalidateQueries({ queryKey: ['facets'] });
        const first = e.payload?.errors?.[0];
        if (!first) {
          lastScanError.current = null;
        } else if (first !== lastScanError.current) {
          lastScanError.current = first;
          toast(`会话扫描：${first}`);
        }
      }),
      // 积分轮询完成（默认每 5 分钟，间隔在「设置 → 定时任务」改）：
      // 账号列表的余额/配额可能变了，静默重取，不弹 toast
      listen<unknown>('credits:round', () => {
        qc.invalidateQueries({ queryKey: ['accounts'] });
        qc.invalidateQueries({ queryKey: ['detail'] });
      }),
      // 任何一条新领取记录（签到 / 活动 / 猫猫旅行）：只刷这个账号的详情与旅行卡
      listen<string>('claim:recorded', (e) => {
        qc.invalidateQueries({ queryKey: ['detail', e.payload] });
        qc.invalidateQueries({ queryKey: ['activity', e.payload] });
        qc.invalidateQueries({ queryKey: ['accounts'] });
        qc.invalidateQueries({ queryKey: ['recent-claims'] });
      }),
      // 猫猫旅行定时轮询（每小时一次）：状态、记录、余额和领取台账都可能变。
      // 不弹 toast——每小时一条会盖住真正需要人看的提示，结果在详情卡与日志里。
      listen<unknown>('activity:round', () => {
        qc.invalidateQueries({ queryKey: ['activity'] });
        qc.invalidateQueries({ queryKey: ['detail'] });
        qc.invalidateQueries({ queryKey: ['accounts'] });
      }),
      listen<string>('zcode:claim', (e) => {
        toast(`ZCode 自动领取：${e.payload}`);
      }),
      // 彻底删除在后台任务里跑，结束时推一条结果：刷新两个列表 + 告知结果
      listen<{
        deleted?: number;
        files: number;
        bytes: number;
        refused?: number;
        errors?: string[];
        warnings?: string[];
      }>('session:purge', (e) => {
        qc.invalidateQueries({ queryKey: ['sessions'] });
        qc.invalidateQueries({ queryKey: ['facets'] });
        const failed = e.payload.errors?.length ?? 0;
        toast(
          `已彻底删除 ${e.payload.deleted ?? 0} 条（${e.payload.files} 个文件）` +
            (e.payload.refused ? `；${e.payload.refused} 条可见会话已跳过` : '') +
            // 失败条数 + 第一条原因：只说「0 条」等于没说
            (failed ? `；${failed} 条失败：${e.payload.errors?.[0] ?? ''}` : ''),
        );
        // 重启客户端失败必须单独说：否则用户只看到「客户端被关了没起来」
        const warning = e.payload.warnings?.[0];
        if (warning) toast(`注意：${warning}`);
      }),
      // 浏览器登录完成（成功/失败都推）：账号库变了，列表与「当前登录」都要重拉
      listen<LoginOutcome>('account:login', (e) => {
        qc.invalidateQueries({ queryKey: ['accounts'] });
        qc.invalidateQueries({ queryKey: ['current'] });
        toast(e.payload.ok ? e.payload.message : `登录未完成：${e.payload.message}`);
      }),
      // 启动后台扫描完成：账号库与各客户端当前登录可能变了，列表要重拉
      listen<unknown>('accounts:scanned', () => {
        qc.invalidateQueries({ queryKey: ['accounts'] });
        qc.invalidateQueries({ queryKey: ['current'] });
      }),
      listen<LogLine>('log:line', (e) => {
        qc.setQueryData<LogLine[]>(['log-live'], (old) => [...(old ?? []).slice(-299), e.payload]);
      }),
    ];
    // 启动就把唯一一个「重」查询预热掉：技能库要遍历各软件的技能目录（实测 ~0.85s），
    // 其余查询都在 20ms 内。等用户点开「技能」再去读，那半秒多的骨架就躲不掉了。
    // 延 200ms 是让首屏那几条毫秒级查询先跑完，别在启动瞬间跟它们抢。
    const warm = window.setTimeout(() => {
      void qc.prefetchQuery({ queryKey: ['hub'], queryFn: () => queryCmd('hub_snapshot') });
    }, 200);
    return () => {
      window.clearTimeout(warm);
      unbinds.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, [qc]);
  return null;
}

export function AppProviders({ children }: { children: React.ReactNode }) {
  return (
    <QueryClientProvider client={queryClient}>
      <EventBridges />
      {children}
    </QueryClientProvider>
  );
}
