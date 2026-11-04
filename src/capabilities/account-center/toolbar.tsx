// 账号中心顶栏：扫描 / 添加 / 刷积分 / 批量签到 + 添加账号对话框。
//
// 批量签到是任务化的：进度由 tasks 轮询给，这里只负责把「正在跑」画出来，
// 不在前端猜完成时刻。
//
// 「添加账号」统一走浏览器登录（v1 的做法）：选平台 → 开官方登录页 → 轮询入库。
// 客户端全程不参与，所以弹窗只留一个下拉框 + 一个按钮；借客户端那条路退成失败时的兜底。
import { useCallback, useEffect, useState } from 'react';
import { queryCmd, runCmd } from '@/lib/cmd';
import { listen } from '@tauri-apps/api/event';
import { Copy, ExternalLink, Link } from 'lucide-react';
import { Button, cn } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Select } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { errText } from '@/lib/ipc';
import type { AddOutcome, LoginOutcome, TaskView } from '@/lib/types';

/** 可添加的平台。顺序 = 下拉框顺序；`hint` 是登录面板里的那句话。 */
const PLATFORMS: {
  key: string;
  label: string;
  mark: string;
  hint: string;
}[] = [
  {
    key: 'workbuddy',
    label: 'WorkBuddy 国内版',
    mark: 'W',
    hint: '点开链接，或用微信 / QQ 扫码在浏览器里完成授权；页面也支持手机号登录。',
  },
  {
    key: 'workbuddy_intl',
    label: 'WorkBuddy 国际版',
    mark: 'W',
    hint: '点开链接，或用手机扫码在浏览器里完成授权；页面也支持手机号登录。',
  },
  {
    key: 'qoder',
    label: 'Qoder CN',
    mark: 'Q',
    hint: '点开链接，或用 Qoder App 扫码授权；页面也支持手机号登录。',
  },
  {
    key: 'qoder_intl',
    label: 'Qoder 国际版',
    mark: 'Q',
    hint: '点开链接，或用 Qoder App 扫码授权；页面也支持邮箱登录。',
  },
  {
    key: 'zcode',
    label: 'ZCode',
    mark: 'Z',
    hint: '点开链接，用 BigModel（智谱）或 z.ai 账号登录授权。',
  },
];

const platformOf = (key: string) => PLATFORMS.find((p) => p.key === key) ?? PLATFORMS[0];

export function AccountToolbar({
  busy,
  batchTask,
  onScan,
  onAdd,
  onRefresh,
  onCheckin,
}: {
  busy: string | null;
  batchTask?: TaskView;
  onScan: () => void;
  onAdd: () => void;
  onRefresh: () => void;
  onCheckin: () => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      {/* 主按钮给「添加账号」：这是这个页面唯一的正经工作流。
          「扫描客户端」退成普通按钮——它只是同步入口，切回窗口时还会自动跑一次。 */}
      <Button variant="primary" onClick={onAdd} disabled={!!busy}>
        添加账号
      </Button>
      <Button onClick={onScan} loading={busy === 'scan'} disabled={!!busy}>
        扫描客户端
      </Button>
      <Button onClick={onRefresh} loading={busy === 'credits'} disabled={!!busy}>
        刷新积分
      </Button>
      <Button onClick={onCheckin} disabled={!!batchTask}>
        批量签到
      </Button>
      {batchTask && (
        <span className="flex items-center gap-2 text-xs text-muted">
          批量签到中…
          <span className="inline-block h-1.5 w-24 overflow-hidden rounded-full bg-hover">
            <span
              className="block h-full bg-accent transition-all"
              style={{ width: `${Math.round((batchTask.progress ?? 0) * 100)}%` }}
            />
          </span>
        </span>
      )}
    </div>
  );
}

/** 弹窗状态：正在取链接 → 等浏览器授权 / 启动客户端 → 已就绪 → 失败。
 *  没有「选平台」这一态：打开就是链接页，换平台自动重取。 */
type Stage = 'working' | 'waiting' | 'ready' | 'failed';

export function AddAccountDialog({
  open,
  counts,
  onClose,
  onScanned,
}: {
  open: boolean;
  /** 各平台已有账号数（`workbuddy_intl` 与 `workbuddy` 分开算） */
  counts: Record<string, number>;
  onClose: () => void;
  /** 登录/扫描完成后由调用方刷新账号列表 */
  onScanned: () => void;
}) {
  const [platform, setPlatform] = useState('workbuddy');
  const [stage, setStage] = useState<Stage>('working');
  const [outcome, setOutcome] = useState<AddOutcome | null>(null);
  const [loginUrl, setLoginUrl] = useState('');
  const [error, setError] = useState('');
  const [scanning, setScanning] = useState(false);

  // 打开弹窗、或换平台，就立刻要一条新的登录链接——用户不需要再点一次「登录」
  const loadLink = useCallback(async () => {
    setStage('working');
    setError('');
    setLoginUrl('');
    try {
      const info = await queryCmd('account_login_start', { platform });
      setLoginUrl(info.url);
      setOutcome({
        platform,
        label: info.label,
        exe: '',
        backup: '（没有改动客户端登录态）',
        restarted: false,
        window: true,
      });
      setStage('waiting');
    } catch (e) {
      setError(errText(e));
      setStage('failed');
    }
  }, [platform]);

  useEffect(() => {
    if (!open) return;
    setScanning(false);
    setOutcome(null);
    void loadLink();
  }, [open, loadLink]);

  /** 兜底：借客户端登录（关客户端 → 备份登录态 → 重新打开停在登录页）。 */
  const startViaClient = async () => {
    setStage('working');
    setError('');
    try {
      const result = await queryCmd('account_add_prepare', { platform });
      setOutcome(result);
      setStage('ready');
    } catch (e) {
      setError(errText(e));
      setStage('failed');
    }
  };

  // 浏览器登录是后台任务：登录完成后端推 account:login，弹窗据此收尾
  useEffect(() => {
    if (stage !== 'waiting') return;
    let alive = true;
    const un = listen<LoginOutcome>('account:login', (e) => {
      if (!alive) return;
      if (e.payload.ok) {
        onScanned();
        onClose();
      } else {
        setError(e.payload.message);
        setStage('failed');
      }
    });
    return () => {
      alive = false;
      void un.then((fn) => fn()).catch(() => {});
    };
  }, [stage, onScanned, onClose]);

  const scanNow = async () => {
    setScanning(true);
    try {
      const report = await runCmd('account_scan');
      if (report === undefined) return;
      toast(`扫描完成：${report.scanned} 个账号（新增 ${report.added}）`);
      onScanned();
      onClose();
    } finally {
      setScanning(false);
    }
  };

  const openManually = async () => {
    
      const focused = await runCmd('account_open_client', { platform });

      if (focused === undefined) return;
      toast(
        focused
          ? '客户端已打开，登录完成后点「我已登录，扫描客户端」'
          : '客户端已启动，但窗口没到前台——点系统托盘里的图标打开它',
      );
      setStage('ready');
      setOutcome({
        platform,
        label: platformOf(platform).label,
        exe: '',
        backup: '（本次没有改动登录态）',
        restarted: true,
        window: focused,
      });
    
  };

  const picked = platformOf(platform);

  const copyUrl = async () => {
    await navigator.clipboard.writeText(loginUrl).catch(() => {});
    toast('登录链接已复制');
  };

  const openInBrowser = () => {
    void queryCmd('open_external', { url: loginUrl }).catch((e) => toast(errText(e)));
  };

  return (
    <Dialog
      open={open}
      title={`添加账号 · ${picked.label}`}
      subtitle={picked.hint}
      onClose={onClose}
      footer={
        stage === 'failed' ? (
          <>
            <Button variant="ghost" onClick={startViaClient}>
              改用客户端登录
            </Button>
            <Button variant="ghost" onClick={() => void loadLink()}>
              重试
            </Button>
            <Button onClick={onClose}>关闭</Button>
          </>
        ) : stage === 'ready' ? (
          <>
            <Button variant="ghost" onClick={openManually}>
              只打开客户端
            </Button>
            <Button variant="primary" onClick={scanNow} loading={scanning}>
              我已登录，扫描客户端
            </Button>
            <Button onClick={onClose}>关闭</Button>
          </>
        ) : (
          <Button className="w-full" onClick={onClose}>
            取消
          </Button>
        )
      }
    >
      <div className="flex flex-col gap-3">
        <label className="flex flex-col gap-1.5">
          <span className="text-xs text-muted">平台</span>
          <Select
            value={platform}
            onChange={(e) => setPlatform(e.target.value)}
            className="max-w-none"
          >
            {PLATFORMS.map((item) => {
              const count = counts[item.key] ?? 0;
              return (
                <option key={item.key} value={item.key}>
                  {item.label}
                  {count > 0 ? `（已有 ${count} 个）` : '（还没有账号）'}
                </option>
              );
            })}
          </Select>
        </label>

        {stage === 'working' && (
          <p className="flex items-center gap-2 rounded-md border border-line bg-card px-3 py-3 text-xs text-muted">
            <Spinner />
            正在生成 {picked.label} 的登录链接…
          </p>
        )}

        {stage === 'waiting' && (
          <>
            <div className="flex w-full items-start gap-1.5 rounded-md bg-hover px-2.5 py-2 text-xs text-muted">
              <Link size={12} className="mt-0.5 shrink-0" />
              <span className="line-clamp-2 break-all">{loginUrl}</span>
            </div>
            <div className="flex gap-2">
              <Button variant="primary" size="sm" onClick={openInBrowser}>
                <ExternalLink size={13} />
                在浏览器打开
              </Button>
              <Button size="sm" onClick={copyUrl}>
                <Copy size={13} />
                复制链接
              </Button>
            </div>
            <p className="flex items-center gap-1.5 text-xs text-muted">
              <Spinner />
              等待授权完成：在浏览器里登录后，这里会自动收尾
            </p>
          </>
        )}

        {stage === 'failed' && (
          <p className="whitespace-pre-wrap rounded-md border border-line border-l-[3px] border-l-bad bg-card p-2 text-sm text-fg2">
            {error}
          </p>
        )}

        {stage === 'ready' && (
          <ol className="flex flex-col gap-1.5 text-sm text-fg2">
            {outcome?.window === false && (
              <li className="rounded-md border border-line border-l-[3px] border-l-warn bg-card p-2 text-xs">
                客户端进程已经起来了，但主窗口没有到前台——多半收进了系统托盘。
                点任务栏右下角的 {outcome?.label ?? '客户端'} 图标就能打开登录页。
              </li>
            )}
            <li>1. 在客户端窗口里用要添加的账号登录。</li>
            <li>2. 登录完成后回到这里点「我已登录，扫描客户端」。</li>
            <li className="break-all text-xs text-muted">
              旧登录态已备份到 {outcome?.backup ?? '—'}，想切回去随时可以在账号上点「切换登录」。
            </li>
          </ol>
        )}
      </div>
    </Dialog>
  );
}

/** 12px 旋转指示器：与按钮内的 spinner 同一套画法。 */
function Spinner() {
  return (
    <svg
      className={cn('h-3.5 w-3.5 shrink-0 animate-spin text-accent')}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
    >
      <circle cx="12" cy="12" r="10" stroke="currentColor" strokeOpacity="0.25" strokeWidth="4" />
      <path d="M22 12a10 10 0 0 0-10-10" stroke="currentColor" strokeWidth="4" strokeLinecap="round" />
    </svg>
  );
}
