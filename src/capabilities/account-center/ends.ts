// 「客户端的登录状态」的纯数据层：CodeBuddy 系两个端现在登的是谁。
// 判定留在这里、渲染在页面：逻辑不散进 JSX，也就测得动。
import type { AccountView, CodebuddyTargetStatus } from '@/lib/types';

/** 端的登录状态。三档之外没有第四种可能：装没装、登没登。 */
export type EndState = 'signed_in' | 'signed_out' | 'absent';

export type EndRow = {
  /** `software` / `idea`（与切换命令的 target 同值） */
  key: string;
  /** 端名，直接占平台列：`CodeBuddy IDE` / `CodeBuddy 插件` */
  label: string;
  /** 这个端现在登的号：库里有同一个号就用库里的显示名，否则为 null */
  name: string | null;
  /** 客户端自己记的昵称：库里还没有这个号时用它，至少认得出是谁 */
  clientName: string | null;
  /** 账号库里同一个号的行；库里没有则为 null */
  ownerIid: string | null;
  state: EndState;
  /**
   * 端登的是国际版账号。
   *
   * 两端写的都是国内产品线的授权域，国际版票据写进去只会得到一份登不上的会话，
   * 所以这一行只做展示、不给切换入口。
   */
  intl: boolean;
};

/** iid → 平台内 uid：`workbuddy:u1` → `u1`。客户端会话只记 uid。 */
function uidOf(iid: string) {
  return iid.slice(iid.indexOf(':') + 1);
}

/**
 * 两个端的登录状态，**全部保留**（含没装、没登录的）。
 *
 * 早先的实现把「没装 / 没登录」的端直接丢掉，于是界面上看不到这一端到底有没有登录——
 * 而「客户端现在登的是谁」正是用户切号前唯一需要知道的事。三档状态照实说。
 *
 * 三端同号：uid 相同就是同一个号，名字直接用账号库那一行（改名、脱敏跟着走）。
 * 国际版的端属于另一套账号体系（uid 撞车也不是同一个号），不参与归属。
 */
export function endRows(rows: AccountView[], targets: CodebuddyTargetStatus[]): EndRow[] {
  const byUid = new Map<string, AccountView>();
  for (const row of rows) {
    if (row.platform === 'workbuddy' && !row.iid.endsWith('@intl')) byUid.set(uidOf(row.iid), row);
  }

  return targets.map((target) => {
    const intl = target.edition === 'intl';
    // 国际版的端登的是另一套账号体系：uid 撞车也不是同一个号，不做归属
    const owner = !intl && target.current_uid ? byUid.get(target.current_uid) : undefined;
    const state: EndState = target.current_uid
      ? 'signed_in'
      : target.installed
        ? 'signed_out'
        : 'absent';
    return {
      key: target.key,
      label: target.label,
      name: owner?.name ?? null,
      clientName: target.current_name ?? null,
      ownerIid: owner?.iid ?? null,
      state,
      intl,
    };
  });
}

/**
 * 这一端可以切到哪些号：同平台（WorkBuddy 国内版）账号全集。
 *
 * 这里**不筛「有没有档案」**：筛掉之后用户只能得到一个空列表和一句「没有可切的账号」，
 * 反而看不出想切的那个号其实就在库里、只是缺客户端档案。档案是否可用由后端
 * `account_switchable` 判定，界面据此把不可写的候选列出来并写明原因。
 */
export function endSwitchCandidates(rows: AccountView[], end: EndRow): AccountView[] {
  return rows.filter(
    (row) =>
      row.platform === 'workbuddy' &&
      !row.iid.endsWith('@intl') &&
      row.iid !== end.ownerIid,
  );
}

/** 端状态的短文案：状态行与弹窗共用同一套说法。 */
export function endStateLabel(end: EndRow): string {
  if (end.state === 'absent') return '未安装';
  if (end.state === 'signed_out') return '未登录';
  return end.name ?? end.clientName ?? '已登录';
}
