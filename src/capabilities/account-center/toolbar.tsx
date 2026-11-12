// 账号中心顶栏：扫描 / 添加 / 刷积分 / 批量签到 + 添加账号对话框。
//
// 批量签到是任务化的：进度由 tasks 轮询给，这里只负责把「正在跑」画出来，
// 不在前端猜完成时刻。
//
// 「添加账号」统一走浏览器登录（v1 的做法）：选平台 → 开官方登录页 → 轮询入库。
// 客户端全程不参与，所以弹窗只留一个下拉框 + 一个按钮；借客户端那条路退成失败时的兜底。
import { useCallback, useEffect, useRef, useState } from 'react';
import { queryCmd, runCmd } from '@/lib/cmd';
import { listen } from '@/lib/events';
import { Check, Copy, ExternalLink, Link } from 'lucide-react';
import { Button, cn } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Select } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { errText } from '@/lib/ipc';
import type { AccountView, AddOutcome, LoginOutcome } from '@/lib/types';

/** 可添加的平台。顺序 = 下拉框顺序。 */
const PLATFORMS: { key: string; label: string }[] = [
  { key: 'workbuddy', label: 'WorkBuddy 国内版' },
  { key: 'workbuddy_intl', label: 'WorkBuddy 国际版' },
  { key: 'qoder', label: 'Qoder CN' },
  { key: 'qoder_intl', label: 'Qoder 国际版' },
  { key: 'zcode', label: 'ZCode' },
];

const platformOf = (key: string) => PLATFORMS.find((p) => p.key === key) ?? PLATFORMS[0];

export function AccountToolbar({
  busy,
  onAdd,
  onRefresh,
  onRefreshState,
  onRefreshCredentials,
  onCheckin,
  onDiagnose,
}: {
  busy: string | null;
  onAdd: () => void;
  onRefresh: () => void;
  /** 重扫客户端登录态 + 校正到期时间：让本地库追上客户端的实际登录情况 */
  onRefreshState: () => void;
  /** 逐个账号刷新凭据：不管是否过期都重扫，把最新凭据与有效期写回 */
  onRefreshCredentials: () => void;
  onCheckin: () => void;
  /** 批量只读诊断：不写库、不触发签到 */
  onDiagnose: () => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      {/* 主按钮给「添加账号」：这是这个页面唯一的正经工作流。
          账号同步已改为启动时后台自动扫描，这里不再放手动「扫描客户端」按钮。 */}
      <Button variant="primary" onClick={onAdd} disabled={!!busy}>
        添加账号
      </Button>
      <Button onClick={onRefresh} loading={busy === 'credits'} disabled={!!busy}>
        {busy === 'credits' ? '刷新中…' : '刷新积分'}
      </Button>
      {/* 与「刷新积分」的区别：积分刷的是平台余额，这里刷的是**本地库与客户端登录态** */}
      <Button
        onClick={onRefreshState}
        loading={busy === 'refresh-state'}
        disabled={!!busy}
        title="重扫客户端登录态，并用凭据包校正各账号的到期时间与「当前登录」标记"
      >
        {busy === 'refresh-state' ? '刷新中…' : '刷新账号状态'}
      </Button>
      {/* 与「刷新账号状态」的区别：那个是全量重扫；这个针对当前筛选的一批账号，
          逐个给出「更新了 / 已是最新 / 需重登」的结论，且**不会因为未过期就跳过** */}
      <Button
        onClick={onRefreshCredentials}
        loading={busy === 'refresh-credentials'}
        disabled={!!busy}
        title="对当前筛选的账号逐个重扫客户端，把最新凭据与有效期写回本地库（不管是否过期都会刷）"
      >
        {busy === 'refresh-credentials' ? '刷新中…' : '刷新凭据'}
      </Button>
      <Button onClick={onCheckin} loading={busy === 'checkin'} disabled={!!busy}>
        {busy === 'checkin' ? '签到中…' : '批量签到'}
      </Button>
      <Button
        onClick={onDiagnose}
        loading={busy === 'diagnose'}
        disabled={!!busy}
        title="对当前筛选出的账号做一次只读诊断（并发 3），结果进日志"
      >
        {busy === 'diagnose' ? '诊断中…' : '批量诊断'}
      </Button>
    </div>
  );
}

/** 弹窗状态：正在取链接 → 等浏览器授权 / 启动客户端 → 已就绪 → 失败。
 *  没有「选平台」这一态：打开就是链接页，换平台自动重取。 */
type Stage = 'working' | 'waiting' | 'ready' | 'failed';

// 弹窗视觉约束（refactoring-ui brief）：
//   分级：平台下拉 = 上下文；登录链接 = 待办对象；「在浏览器打开」= 唯一主操作，
//         「复制链接」= 次级。两者等宽、同高成对出现，避免一高一矮的拼贴感。
//   令牌：text-xs 12 / text-sm 13、radius-md、组内 gap-2 组间 gap-4、accent 只留给主按钮。
//   开网页是用户的选择：这里不自动调用 open_external，只把链接摆出来等那一次点击。

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
        restarted: true,
        window: focused,
      });
    
  };

  const picked = platformOf(platform);

  return (
    <Dialog
      open={open}
      title={`添加账号 · ${picked.label}`}
      onClose={onClose}
      footer={
        stage === 'failed' ? (
          <>
            <Button variant="ghost" onClick={startViaClient}>
              改用客户端登录
            </Button>
            <Button variant="ghost" onClick={onClose}>
              关闭
            </Button>
            <Button variant="primary" onClick={() => void loadLink()}>
              重试
            </Button>
          </>
        ) : stage === 'ready' ? (
          <>
            <Button variant="ghost" onClick={openManually}>
              只打开客户端
            </Button>
            <Button variant="ghost" onClick={onClose}>
              关闭
            </Button>
            <Button variant="primary" onClick={scanNow} loading={scanning}>
              我已登录，扫描客户端
            </Button>
          </>
        ) : (
          // 等待态的主操作在正文里（打开网页），页脚只留一个退出口
          <Button variant="ghost" onClick={onClose}>
            取消
          </Button>
        )
      }
    >
      <div className="flex flex-col gap-4">
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
          <p className="flex items-center gap-2 text-xs text-muted">
            <Spinner />
            正在生成 {picked.label} 的登录链接…
          </p>
        )}

        {stage === 'waiting' && <LoginLink url={loginUrl} />}

        {stage === 'failed' && (
          <p className="whitespace-pre-wrap rounded-md border border-line border-l-[3px] border-l-bad bg-card px-3 py-2 text-sm text-fg2">
            {error}
          </p>
        )}

        {stage === 'ready' && (
          <ol className="flex flex-col gap-1.5 text-sm text-fg2">
            {outcome?.window === false && (
              <li className="rounded-md border border-line border-l-[3px] border-l-warn bg-card px-3 py-2 text-xs">
                客户端进程已经起来了，但主窗口没有到前台——多半收进了系统托盘。
                点任务栏右下角的 {outcome?.label ?? '客户端'} 图标就能打开登录页。
              </li>
            )}
            <li>1. 在客户端窗口里用要添加的账号登录。</li>
            <li>2. 登录完成后回到这里点「我已登录，扫描客户端」。</li>
            <li className="text-xs text-muted">
              想切回原来的账号，随时可以在账号列表上点「切换登录」。
            </li>
          </ol>
        )}
      </div>
    </Dialog>
  );
}

/**
 * 打开登录页：优先**无痕窗口**（不复用浏览器已有会话），
 * 找不到支持无痕参数的浏览器时退回默认浏览器。返回浏览器名，null = 走的默认浏览器。
 */
async function openLoginPage(url: string): Promise<string | null> {
  try {
    return await queryCmd('open_private_browser', { url });
  } catch {
    try {
      await queryCmd('open_external', { url });
      return null;
    } catch (e) {
      toast(errText(e));
      return null;
    }
  }
}

/**
 * 登录链接：一条一等公民的「值」——单行截断 + 可整段选中，悬停给全文。
 * 「添加账号」与「重新登录」共用，避免各写一套复制反馈。
 *
 * `autoOpen` 只给「重新登录」用：用户的意图就是去登录，不该再让他在对话框里找按钮。
 */
function LoginLink({ url, autoOpen = false }: { url: string; autoOpen?: boolean }) {
  // 复制反馈：剪贴板写入没有任何系统提示，按钮自己「已复制」一秒半才算闭环
  const [copied, setCopied] = useState(false);
  // 已用哪个浏览器打开过；null = 还没打开
  const [opened, setOpened] = useState<string | null>(null);
  const timer = useRef<number | null>(null);
  // 同一个链接只自动打开一次：StrictMode 会双跑 effect，重复弹窗口用户会以为点错了
  const autoOpened = useRef('');

  useEffect(
    () => () => {
      if (timer.current) window.clearTimeout(timer.current);
    },
    [],
  );

  const open = async () => {
    const browser = await openLoginPage(url);
    setOpened(browser ?? '默认浏览器');
  };

  useEffect(() => {
    if (!autoOpen || !url || autoOpened.current === url) return;
    autoOpened.current = url;
    void open();
  }, [autoOpen, url]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(url);
    } catch {
      toast('复制失败，请手动选中链接复制');
      return;
    }
    setCopied(true);
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2 rounded-md border border-line bg-card px-3 py-2">
        <Link size={13} className="shrink-0 text-muted" />
        <span className="min-w-0 flex-1 truncate text-xs text-fg2 select-all" title={url}>
          {url}
        </span>
      </div>
      {/* 两个快捷动作等宽同高：主次靠底色区分，不靠大小差 */}
      <div className="grid grid-cols-2 gap-2">
        <Button variant="primary" className="w-full py-2" onClick={() => void open()}>
          <ExternalLink size={14} />
          {opened ? '重新打开登录页' : '在无痕窗口打开'}
        </Button>
        <Button className="w-full py-2" onClick={copy}>
          {copied ? <Check size={14} /> : <Copy size={14} />}
          {copied ? '已复制' : '复制链接'}
        </Button>
      </div>
      {opened && (
        <p className="text-xs text-muted">
          已在 <span className="text-fg">{opened}</span>{' '}
          的无痕窗口打开。那边没有你平时的登录状态，登进去的一定是当场输入的这个账号。
        </p>
      )}
    </div>
  );
}

/**
 * 重新登录：给已有账号补一次凭据（详情页的「重新登录」）。
 *
 * 复用「添加账号」那条浏览器登录链路，差别只在平台由账号决定、不再下拉选择。
 * 登录成功后凭据由后端自动保存；这里必须把「登进来的到底是不是这个账号」说清楚——
 * 登成别的号会新增一行，不提示的话用户会以为原账号已经修好了。
 */
export function ReloginDialog({
  target,
  onClose,
  onDone,
}: {
  /** 要补凭据的账号；null = 关闭 */
  target: AccountView | null;
  onClose: () => void;
  /** 凭据已落库：调用方刷新列表与详情 */
  onDone: () => void;
}) {
  const [stage, setStage] = useState<Stage>('working');
  const [loginUrl, setLoginUrl] = useState('');
  const [error, setError] = useState('');
  const iid = target?.iid ?? '';
  const name = target?.name ?? '';
  const platformLabel = target?.platform_label ?? '';

  const loadLink = useCallback(async () => {
    if (!iid) return;
    setStage('working');
    setError('');
    setLoginUrl('');
    try {
      const info = await queryCmd('account_relogin_start', { iid });
      setLoginUrl(info.url);
      setStage('waiting');
    } catch (e) {
      setError(errText(e));
      setStage('failed');
    }
  }, [iid]);

  useEffect(() => {
    if (!iid) return;
    void loadLink();
  }, [iid, loadLink]);

  // 登录是后台任务：完成后端推 account:login，这里据此收尾并刷新
  useEffect(() => {
    if (stage !== 'waiting') return;
    let alive = true;
    const un = listen<LoginOutcome>('account:login', (e) => {
      if (!alive) return;
      const outcome = e.payload;
      if (!outcome.ok) {
        setError(outcome.message);
        setStage('failed');
        return;
      }
      toast(
        outcome.iid === iid
          ? `已重新登录并保存凭据：${name}`
          : `${outcome.message}\n这次登录的不是「${name}」，已作为新账号入库`,
      );
      onDone();
      onClose();
    });
    return () => {
      alive = false;
      void un.then((fn) => fn()).catch(() => {});
    };
  }, [stage, iid, name, onDone, onClose]);

  return (
    <Dialog
      open={!!target}
      title={target ? `重新登录 · ${target.name}` : '重新登录'}
      onClose={onClose}
      footer={
        stage === 'failed' ? (
          <>
            <Button variant="ghost" onClick={onClose}>
              关闭
            </Button>
            <Button variant="primary" onClick={() => void loadLink()}>
              重试
            </Button>
          </>
        ) : (
          <Button variant="ghost" onClick={onClose}>
            取消
          </Button>
        )
      }
    >
      <div className="flex flex-col gap-4">
        {stage === 'working' && (
          <p className="flex items-center gap-2 text-xs text-muted">
            <Spinner />
            正在生成「{platformLabel}」的登录链接…
          </p>
        )}

        {stage === 'waiting' && (
          <div className="flex flex-col gap-2">
            <p className="text-xs text-muted">
              正在无痕窗口里打开登录页。请在新窗口用「{name}」完成登录；凭据会自动保存，
              不需要再点「扫描客户端」。
            </p>
            {/* 用户点「重新登录」就是要登，链接一出来就直接把窗口拉起来 */}
            <LoginLink url={loginUrl} autoOpen />
          </div>
        )}

        {stage === 'failed' && (
          <p className="whitespace-pre-wrap rounded-md border border-line border-l-[3px] border-l-bad bg-card px-3 py-2 text-sm text-fg2">
            {error}
          </p>
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
