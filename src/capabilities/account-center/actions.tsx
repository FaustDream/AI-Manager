// 列表操作列与它的两个确认框：切换登录、恢复对话。
//
// 这些动作都写客户端文件（失败要重做），所以一律先确认再执行；真正调用在 page 里，
// 这里只负责「按什么条件可点、点了之后长什么样」。
//
// 操作列一律用**图标**（IconPark）：六个中文按钮挤在一起会把列撑到 290px，
// 换成图标后只要 190px，文字说明交给 title 悬浮提示。每个动作永远占住自己的槽位，
// 不可用时置灰而不是消失——少一个按钮后面的整体左移，十行看下来是锯齿。
import type { ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Tag } from '@/components/ui';
import { IconDelete, IconHistory, IconPower, IconSwitch } from '@/components/ui/icons';
import type { AccountView } from '@/lib/types';

/** 五个槽位的固定宽度（px）：当前标签 / 切换 / 恢复对话 / 启停 / 删除。 */
export const ACTION_SLOTS = [46, 28, 28, 28, 28] as const;
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
  danger,
}: {
  children: ReactNode;
  onClick: () => void;
  disabled?: boolean;
  title?: string;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      className={`flex h-6 w-7 items-center justify-center rounded-sm transition-colors ${
        disabled
          ? 'cursor-default text-muted opacity-40'
          : danger
            ? 'text-bad hover:bg-bad/10'
            : 'text-accent hover:bg-accent/10'
      }`}
      disabled={disabled}
      title={title}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

export function RowActions({
  row,
  isCurrent,
  switchable,
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
  busy: boolean;
  onSwitch: () => void;
  onRestore: () => void;
  onDelete: () => void;
  onToggleEnabled: () => void;
}) {
  const canRestore = row.platform === 'workbuddy';
  return (
    <div className="flex justify-end">
      <div
        className="grid items-center gap-1"
        style={{ gridTemplateColumns: ACTION_SLOTS.map((w) => `${w}px`).join(' ') }}
      >
        {/* 当前账号才挂标签；其余行留等宽空位，保证后面的按钮不左右漂 */}
        <span className="flex justify-center">
          {isCurrent && (
            <Tag tone="ok" className="px-1.5">
              当前
            </Tag>
          )}
        </span>
        <RowAction
          onClick={onSwitch}
          disabled={!row.has_secret || isCurrent || busy || !switchable}
          title={
            isCurrent
              ? '已经是客户端当前登录的账号'
              : !row.has_secret
                ? '这个账号没有保存登录凭据，先在客户端登录一次再扫描'
                : !switchable
                  ? '网页登录的账号不能切换客户端：请在客户端里登录一次该账号（添加账号 → 改用客户端登录），之后即可切换'
                  : '把客户端登录态切换到这个账号'
          }
        >
          <IconSwitch size={15} />
        </RowAction>
        <RowAction
          onClick={onRestore}
          disabled={!canRestore || !isCurrent || busy}
          title={
            !canRestore
              ? '只有 WorkBuddy 支持会话恢复'
              : !isCurrent
                ? '只能恢复当前登录账号的会话：先切换过去'
                : '把本机已有的历史会话关联到这个账号（只改归属，不动正文）'
          }
        >
          <IconHistory size={15} />
        </RowAction>
        <RowAction
          onClick={onToggleEnabled}
          disabled={busy}
          title={
            row.enabled
              ? '停用：不参与自动签到、自动轮询与积分刷新；手动单账号操作仍可执行'
              : '启用：重新参与自动签到与积分轮询'
          }
        >
          <IconPower size={15} />
        </RowAction>
        <RowAction danger onClick={onDelete} disabled={busy} title="删除：只从本机账号库移除，不动客户端登录态">
          <IconDelete size={15} />
        </RowAction>
      </div>
    </div>
  );
}

export function SwitchDialog({
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
      title="切换登录"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>取消</Button>
          <Button variant="primary" onClick={onConfirm} loading={busy}>
            确认切换
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2 text-sm text-muted">
        <p className="whitespace-pre-wrap">
          会自动关闭并重启「{target?.platform_label}」客户端，并写入它的登录态文件；
          切换失败需要重新登录一次。
        </p>
        <p className="rounded-md border border-line bg-card px-2 py-1 text-fg">
          切换到：{target?.name}（{target?.platform_label}）
        </p>
        {target?.platform === 'workbuddy' && (
          <p>WorkBuddy 切换完成后，会把原账号名下的历史会话一并关联到新账号。</p>
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
          把本机已有的项目会话关联到「{target?.name}」：只改会话归属引用，
          正文一个字节都不动，可重复执行，不会产生副本。
        </p>
        <p>会先关闭 WorkBuddy，写客户端库前自动备份整份库文件，完成后重新打开客户端。</p>
      </div>
    </Dialog>
  );
}
