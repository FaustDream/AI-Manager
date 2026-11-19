// 顶栏「退出」：浏览器化后这是停止后端的正式入口（接替托盘右键退出）。
// 流程：confirmAction 确认 → app_quit（后端先回包再延迟退出）→ 告别遮罩。
// 遮罩盖住整页：后端已死，任何按钮都不再有意义，只剩「关闭此页面」一句指引。
import { useState } from 'react';
import { Power } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { confirmAction } from '@/components/ui/confirm';
import { runCmd } from '@/lib/cmd';

export function QuitButton() {
  const [quit, setQuit] = useState(false);
  const [busy, setBusy] = useState(false);

  const doQuit = async () => {
    const ok = await confirmAction({
      title: '退出 AI Manager',
      message: '退出后后端进程结束，定时签到与自动任务都会停止。\n确定退出吗？',
      danger: true,
      okText: '退出',
    });
    if (!ok) return;
    setBusy(true);
    // app_quit 的正常回包是 null；若后端退得太快连回包都断了，
    // 进程同样已经死了——两种情况都盖「已退出」遮罩，不弹错误 toast。
    await runCmd('app_quit', undefined, { onError: () => setQuit(true) });
    setBusy(false);
    setQuit(true);
  };

  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        title="退出 AI Manager（结束后端进程）"
        aria-label="退出 AI Manager"
        loading={busy}
        disabled={busy}
        onClick={() => void doQuit()}
      >
        <Power size={14} />
      </Button>
      {quit && (
        <div className="fixed inset-0 z-50 flex flex-col items-center justify-center gap-3 bg-bg/95 px-6 text-center backdrop-blur-sm">
          <p className="text-base font-semibold text-fg">AI Manager 已退出</p>
          <p className="text-sm text-muted">
            后端进程已结束，定时任务不再运行。可以关闭此页面；
            需要再用时双击 start.cmd 重新启动。
          </p>
        </div>
      )}
    </>
  );
}
