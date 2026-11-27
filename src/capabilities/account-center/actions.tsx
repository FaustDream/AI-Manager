// 列表操作列与它的两个确认框：切换登录、恢复对话。
//
// 这些动作都写客户端文件（失败要重做），所以一律先确认再执行；真正调用在 page 里，
// 这里只负责「按什么条件可点、点了之后长什么样」。
//
// 操作列一律用**图标**（IconPark）：六个中文按钮挤在一起会把列撑到 290px，
// 换成图标后只要 190px，文字说明交给 title 悬浮提示。每个动作永远占住自己的槽位：
// 暂时不可用（没凭据、不是当前账号）**置灰**，平台根本没有这个功能（或用户在设置里
// 关掉了）则**槽位留空不画图标**——功能不存在的图标只是噪音，但宽度必须保持，
// 否则少一个按钮后面的整体左移，十行看下来是锯齿。
import { type ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Tooltip } from '@/components/ui';
import { IconDelete, IconHistory, IconPower, IconSwitch } from '@/components/ui/icons';
import type { AccountView } from '@/lib/types';
import { webLoginUsable } from './columns';

/** 四个槽位的固定宽度（px）：切换 / 恢复对话 / 启停 / 删除。
 *  「当前登录」不占槽位——它由昵称旁的徽章表达；行底色只表示「正在看哪一行」。 */
export const ACTION_SLOTS = [28, 28, 28, 28] as const;
/** 槽位间距（gap-1）与单元格左右内边距，算操作列总宽时要用。 */
export const ACTION_GAPS = 4;
export const ACTION_PAD = 16;

export const OPS_WIDTH =
  ACTION_SLOTS.reduce((sum, w) => sum + w, 0) + ACTION_GAPS * (ACTION_SLOTS.length - 1) + ACTION_PAD;

function RowAction({
  children,
  onClick,
  disabled,
  title,
  label,
  danger,
}: {
  children: ReactNode;
  onClick: () => void;
  disabled?: boolean;
  title?: string;
  /** 读屏用的短名（「切换登录」）。title 是长解释，不适合当无障碍名称。 */
  label: string;
  danger?: boolean;
}) {
  return (
    <Tooltip content={title}>
      <button
        type="button"
        // 点击区 28×28（总纲 P1-11）：图标仍是 15px，但可点面积够手指与触控板命中。
        // 只加高不加宽——OPS_WIDTH 由 ACTION_SLOTS 算出，动宽度会牵动整表列宽。
        className={`flex h-7 w-7 items-center justify-center rounded-sm transition-colors duration-[var(--dur-1)] ${
          disabled
            ? 'cursor-default text-muted opacity-40'
            : danger
              ? 'cursor-pointer text-bad hover:bg-bad/10'
              : 'cursor-pointer text-accent hover:bg-accent/10'
        }`}
        disabled={disabled}
        aria-label={label}
        onClick={onClick}
      >
        {children}
      </button>
    </Tooltip>
  );
}

export function RowActions({
  row,
  isCurrent,
  switchable,
  canSwitch,
  canRestore,
  busy,
  onSwitch,
  onRestore,
  onDelete,
  onToggleEnabled,
}: {
  row: AccountView;
  isCurrent: boolean;
  /** 现在能否切到客户端：网页登录的账号没有登录态档案，禁用并说明 */
  switchable: boolean;
  /** 这个软件有没有「切换账号」入口（设置 → 各软件功能 × 平台能力） */
  canSwitch: boolean;
  /** 这个软件有没有「恢复对话」入口（只有 WorkBuddy 有本机会话库） */
  canRestore: boolean;
  busy: boolean;
  onSwitch: () => void;
  onRestore: () => void;
  onDelete: () => void;
  onToggleEnabled: () => void;
}) {
  return (
    <div className="flex justify-end">
      <div
        className="grid items-center gap-1"
        style={{ gridTemplateColumns: ACTION_SLOTS.map((w) => `${w}px`).join(' ') }}
      >
        {/* 平台没有这个功能、或用户在设置里关掉了：槽位留空、不画图标。
            宽度不变——同一张表里有的行有按钮、有的行没有，按钮一左一右就是锯齿。 */}
        {canSwitch ? (
          <RowAction
            label="切换登录"
            onClick={onSwitch}
            // 「切不了」不再只藏在灰色图标的气泡里：留着可点，点开由弹窗说明原因与补救办法。
            // 只剩真正没有意义的情形置灰——已经就是当前登录、或本机没有这个号的凭据。
            disabled={!row.has_secret || isCurrent || busy}
            title={
              isCurrent
                ? '已经是客户端当前登录的账号'
                : !row.has_secret
                  ? '这个账号没有保存登录凭据，先在客户端登录一次再扫描'
                  : switchable
                    ? '把客户端登录态切换到这个账号'
                    : '点开看为什么还切不了，以及怎么补'
            }
          >
            <IconSwitch size={15} />
          </RowAction>
        ) : (
          <span />
        )}
        {canRestore ? (
          <RowAction
            label="恢复对话"
            onClick={onRestore}
            disabled={!isCurrent || busy}
            title={
              !isCurrent
                ? '只能恢复当前登录账号的会话：先切换过去'
                : '把本平台所有账号的历史会话关联到这个账号（只改归属，不动正文）'
            }
          >
            <IconHistory size={15} />
          </RowAction>
        ) : (
          <span />
        )}
        <RowAction
          label={row.enabled ? '停用账号' : '启用账号'}
          onClick={onToggleEnabled}
          disabled={busy}
          title={
            row.enabled
              ? '停用：不参与定时任务（每日签到、账号状态刷新、积分刷新）；手动单账号操作仍可执行'
              : '启用：重新参与定时任务（每日签到、账号状态刷新、积分刷新）'
          }
        >
          <IconPower size={15} />
        </RowAction>
        <RowAction danger label="删除账号" onClick={onDelete} disabled={busy} title="删除：只从本机账号库移除，不动客户端登录态">
          <IconDelete size={15} />
        </RowAction>
      </div>
    </div>
  );
}

export function SwitchDialog({
  target,
  switchable,
  busy,
  onClose,
  onConfirm,
}: {
  target: AccountView | null;
  /** 本机是否有这个账号完整的客户端档案；没有就只能先登录一次，确认按钮不给点 */
  switchable: boolean;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog
      open={!!target}
      title={switchable ? '切换登录' : '这个账号还切不了'}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{switchable ? '取消' : '知道了'}</Button>
          {switchable && (
            <Button variant="primary" onClick={onConfirm} loading={busy}>
              确认切换
            </Button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-2 text-sm text-muted">
        {switchable ? (
          <>
            <p className="whitespace-pre-wrap">
              会自动关闭并重启「{target?.platform_label}」客户端，并写入它的登录态文件；
              切换失败需要重新登录一次。
            </p>
            <p className="rounded-md border border-line bg-card px-2 py-1 text-fg">
              切换到：{target?.name}（{target?.platform_label}）
            </p>
            {target?.platform === 'workbuddy' && (
              <p>切换完成后会自动把该平台所有账号的历史会话关联到新账号，不需要再点「恢复对话」。</p>
            )}
          </>
        ) : target?.platform === 'workbuddy' ? (
          <>
            <p className="whitespace-pre-wrap text-fg">
              「{target?.name}」没有客户端登录态档案，因此切不过去。
            </p>
            <p className="rounded-md border border-line bg-card px-2 py-1.5">
              档案里装的是客户端那次登录签发的会话与票据，网页登录造不出来。
              在对应版本的 WorkBuddy 客户端里登录一次这个账号（添加账号 → 选 WorkBuddy →
              「启动客户端重新登录」），登录后点「我已登录，刷新凭据」保存档案，之后就能一键切换。
            </p>
            <p>
              网页登录的凭据不是白登的：{target && webLoginUsable(target)}照常可用，
              做不了的只有「切换客户端登录态」这一件。
            </p>
          </>
        ) : (
          <>
            <p className="whitespace-pre-wrap text-fg">
              「{target?.name}」没有保存客户端凭据包，因此切不过去。
            </p>
            <p className="rounded-md border border-line bg-card px-2 py-1.5">
              这个平台的切换是把整份客户端凭据包写回去。在账号详情里对它点「重新授权」
              走一次浏览器登录，凭据包会随授权一起保存，之后就能切。
            </p>
          </>
        )}
      </div>
    </Dialog>
  );
}

export function RestoreDialog({
  target,
  busy,
  onClose,
  onConfirm,
}: {
  target: AccountView | null;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog
      open={!!target}
      title="恢复对话"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>取消</Button>
          <Button variant="primary" onClick={onConfirm} loading={busy}>
            确认恢复
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2 text-sm text-muted">
        <p className="whitespace-pre-wrap">
          把本平台所有账号的历史会话都关联到「{target?.name}」：只改会话归属引用，
          正文一个字节都不动，可重复执行，不会产生副本。
        </p>
        <p>会先关闭 WorkBuddy，写客户端库前自动备份整份库文件，完成后重新打开客户端。</p>
      </div>
    </Dialog>
  );
}
