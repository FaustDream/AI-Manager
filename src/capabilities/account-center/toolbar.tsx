// 账号中心顶栏：扫描 / 添加 / 刷积分 / 批量签到 + 添加账号对话框。
//
// 批量签到是任务化的：进度由 tasks 轮询给，这里只负责把「正在跑」画出来，
// 不在前端猜完成时刻。
//
// 「添加账号」两条路：
//   1. 浏览器登录（多数平台）：选平台 → 开官方登录页 → 轮询入库；
//   2. 粘贴 Cookie（豆包这类没有可借用登录流程的平台）：一份 Cookie = 一个账号，
//      多账号就是多份 Cookie，各自入库、互不覆盖。
import { useCallback, useEffect, useRef, useState } from 'react';
import { queryCmd, runCmd } from '@/lib/cmd';
import { listen } from '@/lib/events';
import { Check, Copy, ExternalLink, Link } from 'lucide-react';
import { Button, cn } from '@/components/ui/button';
import { IconAdd, IconKey, IconMoney } from '@/components/ui/icons';
import { Dialog } from '@/components/ui/dialog';
import { Input, Textarea } from '@/components/ui/form';
import { Hint, Picker, Tooltip } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import { errText } from '@/lib/ipc';
import { anyFeatureOn, featurePlatform, FEATURE_CHECKIN, usePlatformFeatures } from '@/lib/features';
import { useProgress } from '@/lib/progress';
import type { BatchProgress } from '@/lib/progress';
import type { AccountView, AddOutcome, BrowserLoginStatus, LoginOutcome } from '@/lib/types';
import { webLoginUsable } from './columns';

/**
 * 可添加的平台：**固定一份名单**，一家产品一个入口，顺序 = 下拉框顺序。
 *
 * ZCode 只有「智谱」一个入口。z.ai / z.ai CN 两族是早期按授权域拆出来的历史平台键，
 * 只在「已存在的账号」上保留（重新授权走详情页），不再出现在添加列表里——
 * 登录页都指向同一个 zcode 入口，摆出三个选项只会让人以为要多登一次。
 */
const PLATFORMS: { key: string; label: string }[] = [
  { key: 'workbuddy', label: 'WorkBuddy 国内版' },
  { key: 'workbuddy_intl', label: 'WorkBuddy 国际版' },
  { key: 'qoder', label: 'Qoder CN' },
  { key: 'qoder_intl', label: 'Qoder 国际版' },
  { key: 'zcode', label: 'ZCode 智谱' },
  { key: 'doubao', label: '豆包' },
  { key: 'ima', label: 'ima' },
];



/**
 * 用 Cookie 添加的平台：没有可借用的登录流程，只能粘贴一次登录态。
 *
 * 豆包网页走字节 SSO（没有可回调的授权码），电脑版的 Cookie 在 Chromium 加密库里
 * （格式随版本变），所以这里让用户从浏览器复制一次——多账号就是多份 Cookie。
 */
const COOKIE_PLATFORMS = new Set(['doubao']);

const platformOf = (key: string) => PLATFORMS.find((p) => p.key === key) ?? PLATFORMS[0];

export function AccountToolbar({
  busy,
  onAdd,
  onRefreshAndCheckin,
  onRefreshCredentials,
}: {
  busy: string | null;
  onAdd: () => void;
  /** 签到 · 积分：先刷平台余额，再对今天还没签的账号签到；已签的只刷积分 */
  onRefreshAndCheckin: () => void;
  /** 重扫客户端并写回最新凭据、到期时间与当前登录标记 */
  onRefreshCredentials: () => void;
}) {
  // 批量动作的实时读数：后端 progress 事件 → 「3 / 12」。总数只有后端知道，
  // 前端只累加完成数；拿不到（事件丢了）就退回默认省略号文案，不编数字。
  const roundProgress = useProgress('checkin_refresh');
  // 所有软件的签到入口都被关掉时，这个按钮只刷积分——文案要说实话，不承诺签到
  const features = usePlatformFeatures();
  const checkinAnywhere = anyFeatureOn(features.data, FEATURE_CHECKIN);
  return (
    /* 图标 + 一个词：纯图标在几个刷新类动作之间分不出来，纯文字又太长。
       完整口径进 title 气泡。 */
    <div className="flex flex-wrap items-center gap-1.5">
      {/* 主按钮给「添加账号」：这是页面唯一的正经工作流。
          账号同步已改为启动时后台自动扫描，这里不再放手动「扫描客户端」按钮。 */}
      <Tooltip content="客户端登录、网页登录或粘贴 Cookie，登录后保存账号">
        <Button variant="primary" onClick={onAdd} disabled={!!busy}>
          <IconAdd size={14} />
          添加账号
        </Button>
      </Tooltip>
      {/* 签到与积分合并成一个入口：同一批账号、同一份凭据、同一个并发预算，
          分两个按钮用户得记住先点哪个，还会把同一批凭据读两遍。 */}
      <Tooltip
        content={
          busy === 'round'
            ? batchText('签到与积分刷新中', roundProgress)
            : checkinAnywhere
              ? '签到 · 积分：先调平台接口重查余额与配额，再对今天还没签的账号执行签到；今日已签的只刷积分，不重复提交（会联网）'
              : '签到 · 积分：所有软件的签到入口都已在设置里关掉，本次只重查余额与配额（会联网）'
        }
      >
        <Button
          onClick={onRefreshAndCheckin}
          loading={busy === 'round'}
          disabled={!!busy}
        >
          <IconMoney size={14} />
          签到 · 积分
        </Button>
      </Tooltip>
      {/* 与上一个的区别：这个重扫本机客户端，把最新凭据、到期时间与
          「当前登录」标记写回本地库，不查平台余额 */}
      <Tooltip
        content={
          busy === 'refresh-credentials'
            ? '正在重扫客户端并刷新账号…'
            : '刷新凭据：重扫客户端，更新每个账号的最新凭据、到期时间与登录态（不查平台余额）'
        }
      >
        <Button
          onClick={onRefreshCredentials}
          loading={busy === 'refresh-credentials'}
          disabled={!!busy}
        >
          <IconKey size={14} />
          刷新凭据
        </Button>
      </Tooltip>
    </div>
  );
}

/**
 * 批量动作的读数文案：`刷新中 3/12`。
 *
 * 图标按钮没有文字位，进度只能进气泡——但它是「还在走」的实证，不能丢。
 * 没有读数（后端没发 start、或事件在途中丢了）就退回 `刷新中…`，不编数字。
 */
function batchText(verb: string, p: BatchProgress | null): string {
  return p && p.running && p.total > 0 ? `${verb} ${p.done}/${p.total}` : `${verb}…`;
}

/** 弹窗状态：正在取链接 → 等浏览器授权 / 启动客户端 → 已就绪 → 失败。
 *  「粘贴 Cookie」是第四条路（`cookie`），它没有链接可等，整块换成表单。
 *  豆包额外有「浏览器登录」：`doubao` = 二选一入口，`browser` = 独立窗口登录进行中。
 *  没有「选平台」这一态：打开就是链接页（或 Cookie 表单 / 豆包入口），换平台自动切换。 */
type Stage =
  | 'working'
  | 'waiting'
  | 'ready'
  | 'failed'
  | 'cookie'
  | 'doubao'
  | 'browser'
  | 'ima'
  | 'client-choice';

// 弹窗视觉约束（refactoring-ui brief）：
//   分级：平台下拉 = 上下文；登录链接 = 待办对象；「在浏览器打开」= 唯一主操作，
//         「复制链接」= 次级。两者等宽、同高成对出现，避免一高一矮的拼贴感。
//   令牌：text-xs 12 / text-sm 13、radius-md、组内 gap-2 组间 gap-4、accent 只留给主按钮。
//   开网页是用户的选择：这里不自动调用 open_external，只把链接摆出来等那一次点击。

export function AddAccountDialog({
  open,
  counts,
  initialPlatform = 'workbuddy',
  reauthTarget,
  onClose,
  onScanned,
}: {
  open: boolean;
  /** 各平台已有账号数（`workbuddy_intl` 与 `workbuddy` 分开算） */
  counts: Record<string, number>;
  initialPlatform?: string;
  reauthTarget?: AccountView;
  onClose: () => void;
  /** 登录/扫描完成后由调用方刷新账号列表 */
  onScanned: () => void;
}) {
  const [platform, setPlatform] = useState(initialPlatform);
  const [viaClient, setViaClient] = useState(false);
  const requestRef = useRef(0);
  const isWorkBuddy = platform === 'workbuddy' || platform === 'workbuddy_intl';
  const [stage, setStage] = useState<Stage>('working');
  const [outcome, setOutcome] = useState<AddOutcome | null>(null);
  const [loginUrl, setLoginUrl] = useState('');
  const [error, setError] = useState('');
  const [scanning, setScanning] = useState(false);
  // 粘贴 Cookie 那条路：Cookie 是凭据，只在本组件内存里待到提交，不落 localStorage
  const [cookie, setCookie] = useState('');
  const [cookieName, setCookieName] = useState('');
  const [adding, setAdding] = useState(false);
  const [cookieError, setCookieError] = useState('');
  const [browserStatus, setBrowserStatus] = useState<BrowserLoginStatus | null>(null);
  // 授权页是否被打开过：只用于失败归因，不参与渲染，所以用 ref（监听器读它不会有旧值）
  const openedRef = useRef(false);

  // 打开弹窗、或换平台，就立刻要一条新的登录链接——用户不需要再点一次「登录」
  const loadLink = useCallback(async (web = false) => {
    const request = ++requestRef.current;
    setError('');
    setLoginUrl('');
    if ((platform === 'workbuddy' || platform === 'workbuddy_intl') && !web) {
      setStage('client-choice');
      return;
    }
    setViaClient(false);
    // ima 没有网页登录可借：只说明「登录客户端后扫描」，不进入切号流程
    if (platform === 'ima') {
      setStage('ima');
      setError('');
      setLoginUrl('');
      setBrowserStatus(null);
      return;
    }
    // 豆包没有可回调的授权码：给一个「浏览器登录 / 粘贴 Cookie」二选一入口
    if (COOKIE_PLATFORMS.has(platform)) {
      setStage('doubao');
      setError('');
      setLoginUrl('');
      setBrowserStatus(null);
      return;
    }
    setStage('working');
    setError('');
    setLoginUrl('');
    openedRef.current = false; // 换了平台就是新一条链接，归因从零开始算
    try {
      const info = await queryCmd('account_login_start', { platform });
      if (request !== requestRef.current) return;
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
      if (request !== requestRef.current) return;
      setError(errText(e));
      setStage('failed');
    }
  }, [platform]);

  useEffect(() => {
    if (!open) return;
    setScanning(false);
    setOutcome(null);
    void loadLink();
    return () => { requestRef.current += 1; };
  }, [open, loadLink]);

  /** 兜底：借客户端登录（关客户端 → 备份登录态 → 重新打开停在登录页）。 */
  const startViaClient = async () => {
    const request = ++requestRef.current;
    setViaClient(true);
    setStage('working');
    setError('');
    try {
      const result = await queryCmd('account_add_prepare', { platform });
      if (request !== requestRef.current) return;
      setOutcome(result);
      setStage('ready');
    } catch (e) {
      if (request !== requestRef.current) return;
      setError(errText(e));
      setStage('failed');
    }
  };

  // 浏览器登录是后台任务：登录完成后端推 account:login，弹窗据此收尾
  useEffect(() => {
    if (!open || stage !== 'waiting') return;
    let alive = true;
    const un = listen<LoginOutcome>('account:login', (e) => {
      if (!alive) return;
      if (e.payload.ok) {
        if (reauthTarget && e.payload.iid !== reauthTarget.iid) {
          toast(`这次登录的不是「${reauthTarget.name}」，已作为其他账号保存`);
        }
        onScanned();
        onClose();
      } else {
        // 超时归因（P0-4）：同一个「等待超时」有两种完全不同的原因，
        // 用「授权页到底打开过没有」把它们分开——否则用户只能对着超时干瞪眼。
        // 读 ref 而不是 state：监听器只订阅一次，闭包里的 state 会是旧值。
        setError(
          e.payload.message +
            '\n' +
            (openedRef.current
              ? '授权页已打开过：回到那个浏览器窗口完成登录，再点「重新打开登录页」重试。'
              : '授权页从未打开：先确认默认浏览器能正常打开网页，国际版平台还要确认系统代理已开。'),
        );
        setStage('failed');
      }
    });
    return () => {
      alive = false;
      void un.then((fn) => fn()).catch(() => {});
    };
  }, [open, stage, onScanned, onClose, reauthTarget]);

  const scanNow = async () => {
    const request = ++requestRef.current;
    setScanning(true);
    try {
      setError('');
      const report = platform === 'ima'
        ? await runCmd('account_scan_platform', { platform }, { onError: (e) => setError(errText(e)) })
        : await runCmd('account_add_finish', { platform }, { onError: (e) => setError(errText(e)) });
      if (request !== requestRef.current || report === undefined) return;
      if (report.errors.length > 0) {
        toast(report.errors.join('；'));
        return;
      }
      if (report.scanned === 0) {
        setError(`没有在 ${platformOf(platform).label} 客户端找到登录态，请先登录后重试`);
        return;
      }
      if (reauthTarget) {
        const current = await queryCmd('account_current');
        if (request !== requestRef.current) return;
        toast(current.includes(reauthTarget.iid)
          ? `已保存「${reauthTarget.name}」的客户端登录与凭据`
          : `这次客户端登录的不是「${reauthTarget.name}」，已保存当前登录账号`);
      } else {
        toast(`扫描完成：${report.scanned} 个账号（新增 ${report.added}）`);
      }
      onScanned();
      onClose();
    } finally {
      if (request === requestRef.current) setScanning(false);
    }
  };

  /**
   * 粘贴 Cookie 添加账号。
   *
   * 校验放在后端（Cookie 里必须含 sessionid），这里只把失败原因显示在表单里——
   * 弹窗一关用户就得重贴一遍整段 Cookie，代价太高。所以走 `onError` 而不是默认 toast。
   */
  const addByCookie = async () => {
    const raw = cookie.trim();
    if (!raw) {
      setCookieError('请先粘贴 Cookie');
      return;
    }
    setAdding(true);
    setCookieError('');
    try {
      const iid = await runCmd(
        'account_add_cookie',
        { platform, cookie: raw, name: cookieName.trim() },
        { onError: (e) => setCookieError(errText(e)) },
      );
      if (!iid) return;
      toast(`${platformOf(platform).label} 账号已添加，可以刷新积分了`);
      setCookie('');
      setCookieName('');
      onScanned();
      onClose();
    } finally {
      setAdding(false);
    }
  };

  /** 豆包浏览器登录：起一个独立临时窗口，后台轮询 cookie、核验身份并入库。 */
  const startBrowserLogin = async () => {
    setError('');
    setBrowserStatus(null);
    try {
      const status = await queryCmd('doubao_browser_login_start', { browser_id: null });
      setBrowserStatus(status);
      setStage('browser');
    } catch (e) {
      // 找不到浏览器这类即时失败也留在 browser 面板里显示，别跳到「改用客户端登录」的 failed 态
      setBrowserStatus({
        state: 'error',
        message: errText(e),
        browser_name: '',
        account_name: '',
        updated: false,
        active: false,
      });
      setStage('browser');
    }
  };

  /** 取消进行中的浏览器登录：后端会关掉临时窗口并清理，回到二选一入口。 */
  const cancelBrowserLogin = async () => {
    await runCmd('doubao_browser_login_cancel').catch(() => {});
    setBrowserStatus(null);
    setStage('doubao');
  };

  // 浏览器登录是后台任务：状态走 doubao:login 事件，成功/取消据此收尾
  useEffect(() => {
    if (stage !== 'browser') return;
    let alive = true;
    const un = listen<BrowserLoginStatus>('doubao:login', (e) => {
      if (!alive) return;
      const s = e.payload;
      setBrowserStatus(s);
      if (s.state === 'success') {
        toast(s.updated ? `已更新账号：${s.account_name}` : `已添加账号：${s.account_name}`);
        onScanned();
        onClose();
      } else if (s.state === 'cancelled') {
        setBrowserStatus(null);
        setStage('doubao');
      }
    });
    return () => {
      alive = false;
      void un.then((fn) => fn()).catch(() => {});
    };
  }, [open, stage, onScanned, onClose]);

  /** ima：只拉起客户端，仍停在说明态，登录完成后由用户点「扫描客户端」。 */
  const openImaClient = async () => {
    const focused = await runCmd('account_open_client', { platform: 'ima' });
    if (focused === undefined) return;
    toast(
      focused
        ? 'ima 客户端已打开，登录完成后点「扫描客户端」'
        : '已尝试打开 ima 客户端，窗口可能在托盘；登录完成后点「扫描客户端」',
    );
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
      title={reauthTarget ? `重新授权 · ${reauthTarget.name}` : `添加账号 · ${picked.label}`}
      onClose={onClose}
      footer={
        stage === 'client-choice' ? (
          <Button variant="ghost" onClick={onClose}>关闭</Button>
        ) : stage === 'ima' ? (
          <>
            <Button variant="ghost" onClick={onClose}>
              关闭
            </Button>
            <Button variant="ghost" onClick={() => void openImaClient()}>
              打开客户端
            </Button>
            <Button variant="primary" onClick={scanNow} loading={scanning}>
              扫描客户端
            </Button>
          </>
        ) : stage === 'cookie' ? (
          <>
            <Button variant="ghost" onClick={onClose}>
              取消
            </Button>
            <Button
              variant="primary"
              onClick={() => void addByCookie()}
              loading={adding}
              disabled={!cookie.trim()}
            >
              添加账号
            </Button>
          </>
        ) : stage === 'browser' ? (
          <Button variant="ghost" onClick={() => void cancelBrowserLogin()}>
            取消登录
          </Button>
        ) : stage === 'doubao' ? (
          <Button variant="ghost" onClick={onClose}>
            关闭
          </Button>
        ) : stage === 'failed' ? (
          <>
            <Button variant="ghost" onClick={() => isWorkBuddy ? void loadLink(viaClient) : void startViaClient()}>
              {isWorkBuddy && viaClient ? '改用网页登录' : '改用客户端登录'}
            </Button>
            <Button variant="ghost" onClick={onClose}>
              关闭
            </Button>
            <Button variant="primary" onClick={() => viaClient ? void startViaClient() : void loadLink(true)}>
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
            {isWorkBuddy && (
              <Button variant="ghost" onClick={() => void loadLink(true)} disabled={scanning}>改用网页登录</Button>
            )}
            <Button variant="primary" onClick={scanNow} loading={scanning}>
              我已登录，刷新凭据
            </Button>
          </>
        ) : (
          <>
            {isWorkBuddy && stage === 'waiting' && <Button variant="ghost" onClick={() => void loadLink()}>改用客户端登录</Button>}
            <Button variant="ghost" onClick={onClose}>取消</Button>
          </>
        )
      }
    >
      <div className="flex flex-col gap-4">
        <label className="flex flex-col gap-1.5">
          <span className="text-xs text-muted">平台</span>
          <Picker
            label="平台"
            value={platform}
            disabled={!!reauthTarget || stage === 'working' || scanning}
            onValueChange={setPlatform}
            className="max-w-none"
            options={PLATFORMS.map((item) => {
              const count = counts[item.key] ?? 0;
              return {
                value: item.key,
                label: `${item.label}${count > 0 ? `（已有 ${count} 个）` : '（还没有账号）'}`,
              };
            })}
          />
        </label>

        {stage === 'client-choice' && (
          <div className="flex flex-col gap-3">
            <p className="text-xs leading-relaxed text-muted">
              客户端登录后可切换账号；网页登录仅用于{webLoginUsable({ requires_checkin: platform === 'workbuddy' })}，不切换客户端。
            </p>
            <Button variant="primary" onClick={() => void startViaClient()}>启动客户端重新登录</Button>
            <p className="text-xs leading-relaxed text-muted">会关闭并重启对应客户端，先保存当前账号档案，再打开登录页。</p>
            <Button onClick={() => void loadLink(true)}>网页登录（仅查询与活动）</Button>
          </div>
        )}

        {stage === 'ima' && (
          <div className="flex flex-col gap-3">
            <p className="text-xs leading-relaxed text-muted">
              ima 的登录态只保存在本机客户端，本工具不提供网页登录。请安装并登录 ima
              客户端后点「扫描客户端」；如果提示 Cookie 库被占用，请完全退出 ima
              客户端后重试。
            </p>
            <p className="text-xs leading-relaxed text-muted">
              本工具对 ima 只做签到与算力查询，不切换登录。
            </p>
          </div>
        )}

        {stage === 'cookie' && (
          <div className="flex flex-col gap-3">
            <label className="flex flex-col gap-1.5">
              <span className="text-xs text-muted">备注名（可选，用来区分同一平台的多个账号）</span>
              <Input
                value={cookieName}
                onChange={(e) => setCookieName(e.target.value)}
                placeholder="例如：主号 / 小号"
                className="max-w-none"
              />
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-xs text-muted">doubao.com 的 Cookie</span>
              <Textarea
                value={cookie}
                onChange={(e) => setCookie(e.target.value)}
                placeholder="粘贴整段 Cookie（形如 a=1; b=2; sessionid=...）"
                className="min-h-[96px] font-mono text-xs"
                spellCheck={false}
              />
            </label>
            <p className="text-xs leading-relaxed text-muted">
              获取方式：浏览器打开 doubao.com 并登录 → F12 → Network 里任选一条 doubao.com 请求
              → 复制请求头中的 <span className="font-mono">Cookie</span> 整段粘贴进来。
              一个账号一份 Cookie，多开就重复添加一次；本工具对豆包只做只读积分查询，
              不签到、不领取（领取接口会被风控，签到请在豆包客户端里点）。
            </p>
            {cookieError && (
              <p className="whitespace-pre-wrap rounded-md border border-line border-l-[3px] border-l-bad bg-card px-3 py-2 text-sm text-fg2">
                {cookieError}
              </p>
            )}
          </div>
        )}

        {stage === 'doubao' && (
          <div className="flex flex-col gap-3">
            <p className="text-xs leading-relaxed text-muted">
              豆包网页登录走字节 SSO、电脑版登录态存在加密库里，无法像其他平台那样借客户端或回调登录。
              两条路任选：自动起一个独立浏览器窗口登录，或手动粘贴 Cookie。
            </p>
            <Button variant="primary" onClick={() => void startBrowserLogin()}>
              在浏览器里登录（自动）
            </Button>
            <Button
              onClick={() => {
                setError('');
                setBrowserStatus(null);
                setStage('cookie');
              }}
            >
              改用粘贴 Cookie
            </Button>
          </div>
        )}

        {stage === 'browser' && (
          <div className="flex flex-col gap-3">
            {browserStatus &&
              ['starting', 'waiting', 'importing'].includes(browserStatus.state) && (
                <p className="flex items-start gap-2 text-xs text-muted">
                  <Spinner />
                  <span className="whitespace-pre-wrap">{browserStatus.message}</span>
                </p>
              )}
            {browserStatus?.state === 'error' && (
              <p className="whitespace-pre-wrap rounded-md border border-line border-l-[3px] border-l-bad bg-card px-3 py-2 text-sm text-fg2">
                {browserStatus.message}
              </p>
            )}
            <p className="text-xs leading-relaxed text-muted">
              登录成功后账号会自动接入，无需复制 Cookie；用的是临时会话，日常浏览器不受影响。
            </p>
            <Button
              variant="ghost"
              onClick={() => {
                setError('');
                setBrowserStatus(null);
                setStage('cookie');
              }}
            >
              改用粘贴 Cookie
            </Button>
          </div>
        )}

        {stage === 'working' && (
          <p className="flex items-center gap-2 text-xs text-muted">
            <Spinner />
            {viaClient ? `正在保存原账号并启动 ${picked.label} 登录页…` : `正在生成 ${picked.label} 的登录链接…`}
          </p>
        )}

        {stage === 'waiting' && (
          <div className="flex flex-col gap-3">
            {isWorkBuddy && <p className="text-xs text-muted">网页登录可用于{webLoginUsable({ requires_checkin: platform === 'workbuddy' })}，不会切换客户端账号。</p>}
            <LoginLink url={loginUrl} onOpened={() => (openedRef.current = true)} />
          </div>
        )}

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
            <li>2. 登录完成后回到这里点「我已登录，刷新凭据」。</li>
            <li className="text-xs text-muted">会短暂关闭客户端，保存新登录态档案并刷新凭据，然后重新打开。</li>
            {error && <li className="whitespace-pre-wrap text-bad">{error}</li>}
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
 * 「添加账号」与「重新授权」共用，避免各写一套复制反馈。
 *
 * `autoOpen` 只给「重新授权」用：用户的意图就是去登录，不该再让他在对话框里找按钮。
 */
function LoginLink({
  url,
  autoOpen = false,
  onOpened,
}: {
  url: string;
  autoOpen?: boolean;
  /** 授权页真的被打开过一次：调用方据此给超时归因（P0-4） */
  onOpened?: () => void;
}) {
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
    onOpened?.();
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
        <Hint
          content={url}
          focusable={false}
          className="min-w-0 flex-1 truncate text-xs text-fg2 select-all"
        >
          {url}
        </Hint>
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
      {opened ? (
        <p className="text-xs leading-relaxed text-muted">
          已在 <span className="text-fg">{opened}</span>{' '}
          的无痕窗口打开。那边没有你平时的登录状态，登进去的一定是当场输入的这个账号。
          <span className="mt-1 block text-fg2">
            请在那个页面完成登录授权，完成后这里会自动识别并关闭，不用回来点任何按钮。
          </span>
        </p>
      ) : (
        // 没打开过授权页 = 超时的高发原因（默认浏览器异常 / 代理没开）。
        // 与「打开了但没登完」分开说，用户才知道该去修浏览器还是回去登录（P0-4）。
        <p className="text-xs leading-relaxed text-muted">
          点上面的按钮打开登录页，在页面里完成登录授权；
          <span className="text-fg2">完成后这里会自动识别并关闭，不用回来点任何按钮。</span>
          <span className="mt-1 block">
            页面打不开或一直转圈：先检查默认浏览器是否正常、国际版平台还要确认系统代理已开。
          </span>
        </p>
      )}
    </div>
  );
}

/**
 * 重新授权：给已有账号补一次凭据（详情页的「重新授权」）。
 *
 * 复用「添加账号」那条浏览器登录链路，差别只在平台由账号决定、不再下拉选择。
 * 登录成功后凭据由后端自动保存；这里必须把「登进来的到底是不是这个账号」说清楚——
 * 登成别的号会新增一行，不提示的话用户会以为原账号已经修好了。
 */
export function ReauthDialog(props: {
  target: AccountView | null;
  onClose: () => void;
  onDone: () => void;
}) {
  if (props.target?.platform === 'workbuddy') {
    return <AddAccountDialog
      key={props.target.iid}
      open
      counts={{}}
      initialPlatform={featurePlatform(props.target.platform, props.target.iid)}
      reauthTarget={props.target}
      onClose={props.onClose}
      onScanned={props.onDone}
    />;
  }
  return <BrowserReauthDialog {...props} />;
}

function BrowserReauthDialog({
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
      const info = await queryCmd('account_reauth_start', { iid });
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
          ? `已重新授权并保存凭据：${name}`
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
      title={target ? `重新授权 · ${target.name}` : '重新授权'}
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
              正在无痕窗口里打开登录页。请在新窗口用「{name}」完成授权；凭据会自动保存，
              不需要再点「扫描客户端」。
            </p>
            {/* 用户点「重新授权」就是要重新走一次授权，链接一出来就直接把窗口拉起来 */}
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
