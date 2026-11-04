// 全局 Provider：QueryClient（服务端状态）+ 事件桥（§6.3 的前端侧）。
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useQueryClient } from '@tanstack/react-query';
import { toast } from '@/components/ui/toast';
import type { LoginOutcome, RoundResult, TaskView } from '@/lib/types';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: 1, staleTime: 15_000, refetchOnWindowFocus: false },
  },
});

/** 事件桥：后端推 task:state / task:progress / log:line / session:scan / checkin:round / zcode:claim，
 *  前端在这里统一接住，按能力失效对应查询——页面不用自己 listen。 */
function EventBridges() {
  const qc = useQueryClient();
  useEffect(() => {
    const unbinds = [
      listen<TaskView>('task:state', (e) => {
        qc.setQueryData<TaskView[]>(['tasks'], (old) => {
          const rows = (old ?? []).filter((t) => t.id !== e.payload.id);
          return [e.payload, ...rows];
        });
        if (e.payload.state === 'done' || e.payload.state === 'failed') {
          qc.invalidateQueries({ queryKey: ['accounts'] });
        }
      }),
      listen<TaskView>('task:progress', (e) => {
        qc.setQueryData<TaskView[]>(['tasks'], (old) => {
          const rows = (old ?? []).filter((t) => t.id !== e.payload.id);
          return [e.payload, ...rows];
        });
      }),
      listen<RoundResult>('checkin:round', (e) => {
        qc.setQueryData(['daily'], e.payload);
        qc.invalidateQueries({ queryKey: ['accounts'] });
        toast(`每日轮询：${e.payload.message}`);
      }),
      listen<unknown>('session:scan', () => {
        qc.invalidateQueries({ queryKey: ['sessions'] });
        qc.invalidateQueries({ queryKey: ['facets'] });
      }),
      // 每小时积分轮询完成：账号列表的余额/配额可能变了
      listen<unknown>('credits:round', () => {
        qc.invalidateQueries({ queryKey: ['accounts'] });
      }),
      listen<string>('zcode:claim', (e) => {
        toast(`ZCode 自动领取：${e.payload}`);
      }),
      // 会话删除 / 彻底删除在后台任务里跑，结束时推一条结果：刷新列表 + 告知结果
      listen<{ sessions?: number; files: number; bytes: number; errors?: string[] }>(
        'session:delete',
        (e) => {
          qc.invalidateQueries({ queryKey: ['sessions'] });
          qc.invalidateQueries({ queryKey: ['facets'] });
          qc.invalidateQueries({ queryKey: ['trash'] });
          const failed = e.payload.errors?.length ?? 0;
          toast(
            `已删除 ${e.payload.sessions ?? 0} 条（${e.payload.files} 个文件）进回收站` +
              (failed ? `；失败 ${failed}` : ''),
          );
        },
      ),
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
        qc.invalidateQueries({ queryKey: ['trash'] });
        const failed = e.payload.errors?.length ?? 0;
        toast(
          `已彻底删除 ${e.payload.deleted ?? 0} 条（${e.payload.files} 个文件）` +
            (e.payload.refused ? `；${e.payload.refused} 条未归档已跳过` : '') +
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
      listen<unknown>('log:line', (e) => {
        qc.setQueryData<unknown[]>(['log-live'], (old) => [...(old ?? []).slice(-299), e.payload]);
      }),
    ];
    return () => {
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
