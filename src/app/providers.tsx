// 全局 Provider：QueryClient（服务端状态）+ 事件桥（§6.3 的前端侧）。
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useQueryClient } from '@tanstack/react-query';
import { toast } from '@/components/ui/toast';
import type { RoundResult, TaskView } from '@/lib/types';

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
      listen<string>('zcode:claim', (e) => {
        toast(`ZCode 自动领取：${e.payload}`);
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
