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
import { useState, type ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { IconDelete, IconHistory, IconPower, IconSwitch } from '@/components/ui/icons';
import type { AccountView } from '@/lib/types';
import type { EndRow } from './ends';

/** 四个槽位的固定宽度（px）：切换 / 恢复对话 / 启停 / 删除。
 *  「当前登录」不再占槽位——它由行背景表达，文字标签是重复信息。 */
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
    <button
      type="button"
      // 点击区 28×28（总纲 P1-11）：图标仍是 15px，但可点面积够手指与触控板命中。
      // 只加高不加宽——OPS_WIDTH 由 ACTION_SLOTS 算出，动宽度会牵动整表列宽。
      className={`flex h-7 w-7 items-center justify-center rounded-sm transition-colors ${
        disabled
          ? 'cursor-default text-muted opacity-40'
          : danger
            ? 'cursor-pointer text-bad hover:bg-bad/10'
            : 'cursor-pointer text-accent hover:bg-accent/10'
      }`}
      disabled={disabled}
      title={title}
      aria-label={label}
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
                : '把本机已有的历史会话关联到这个账号（只改归属，不动正文）'
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
              ? '停用：不参与自动签到、自动轮询与积分刷新；手动单账号操作仍可执行'
              : '启用：重新参与自动签到与积分轮询'
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

/**
 * 端行的操作：切换这一端的登录、以及透过所属账号做的恢复对话 / 启停。
 *
 * 与账号行共用同一套槽位宽度，顺序也对齐（切换 / 恢复 / 启停），删掉的是「删除」
 * ——端不是账号库里的记录，没有可删的东西，槽位留空保持列宽不跳。
 */
export function EndRowActions({
  canSwitch,
  canRestore,
  canToggle,
  restoreDisabled,
  ownerEnabled,
  busy,
  onSwitch,
  onRestore,
  onToggleEnabled,
}: {
  canSwitch: boolean;
  /** 所属账号存在且有「恢复对话」入口 */
  canRestore: boolean;
  /** 所属账号存在：启停是账号级的开关 */
  canToggle: boolean;
  /** 所属账号不是 WorkBuddy 本体当前登录：恢复对话只能对当前登录号做 */
  restoreDisabled: boolean;
  ownerEnabled: boolean;
  busy: boolean;
  onSwitch: () => void;
  onRestore: () => void;
  onToggleEnabled: () => void;
}) {
  return (
    <div className="flex justify-end">
      <div
        className="grid items-center gap-1"
        style={{ gridTemplateColumns: ACTION_SLOTS.map((w) => `${w}px`).join(' ') }}
      >
        {canSwitch ? (
          <RowAction
            label="切换这一端的登录"
            onClick={onSwitch}
            disabled={busy}
            title="把这个端的登录换成账号库里另一个同平台的号"
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
            disabled={restoreDisabled || busy}
            title={
              restoreDisabled
                ? '只能恢复当前登录账号的会话：先让 WorkBuddy 本体切到这个账号'
                : '把本机已有的历史会话关联到所属账号（只改归属，不动正文）'
            }
          >
            <IconHistory size={15} />
          </RowAction>
        ) : (
          <span />
        )}
        {canToggle ? (
          <RowAction
            label={ownerEnabled ? '停用账号' : '启用账号'}
            onClick={onToggleEnabled}
            disabled={busy}
            title={
              ownerEnabled
                ? '停用所属账号：不参与自动签到、自动轮询与积分刷新；手动操作仍可执行'
                : '启用所属账号：重新参与自动签到与积分轮询'
            }
          >
            <IconPower size={15} />
          </RowAction>
        ) : (
          <span />
        )}
        <span />
      </div>
    </div>
  );
}

/**
 * 端行切账号：先选号，再确认。
 *
 * 候选只给同平台（WorkBuddy 国内版）——端写的是它的登录态档案，别的平台后端直接拒。
 * 选号这一步不能省：账号行的「切换」是「切到这一行」，端行反过来是「这一端切到谁」，
 * 没有默认目标。
 */
export function EndSwitchDialog({
  end,
  candidates,
  switchable,
  busy,
  onClose,
  onConfirm,
}: {
  end: EndRow | null;
  candidates: AccountView[];
  /** 有完整客户端档案、真的能写进去的号：没有档案的候选照样列出，但选中前先说清原因 */
  switchable: Set<string>;
  busy: boolean;
  onClose: () => void;
  onConfirm: (iid: string) => void;
}) {
  const [picked, setPicked] = useState<string | null>(null);
  const notReady = candidates.filter((row) => !switchable.has(row.iid));
  return (
    <Dialog
      open={!!end}
      title={end ? `切换「${end.label}」登录` : '切换登录'}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>取消</Button>
          <Button
            variant="primary"
            disabled={!picked}
            loading={busy}
            onClick={() => picked && onConfirm(picked)}
          >
            确认切换
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2 text-sm text-muted">
        <p className="whitespace-pre-wrap">
          {end ? `把「${end.label}」的登录换成下面选中的账号；` : ''}
          只动这一端的登录态，WorkBuddy 本体与另一端不受影响，写前自动备份。
        </p>
        {candidates.length === 0 ? (
          <p className="rounded-md border border-line bg-card px-2 py-1.5 text-fg">
            账号库里还没有其它 WorkBuddy 国内版账号：先「添加账号」登录一个，或在设置里确认这一端的切换能力已打开。
          </p>
        ) : (
          <div className="flex max-h-64 flex-col gap-1 overflow-y-auto">
            {candidates.map((row) => {
              // 缺档案的候选也列出来：用户要能看到「想切的那个号就在库里」，
              // 而不是对着一个空列表猜自己是不是选错了端
              const ready = switchable.has(row.iid);
              return (
                <button
                  key={row.iid}
                  type="button"
                  disabled={!ready}
                  onClick={() => setPicked(row.iid)}
                  aria-pressed={picked === row.iid}
                  title={
                    ready
                      ? `把这一端切到「${row.name}」`
                      : '本机没有这个账号的客户端登录态档案：先在 WorkBuddy 客户端里登录一次（添加账号 → 改用客户端登录），档案生成后就能切'
                  }
                  className={`flex items-center justify-between gap-2 rounded-md border px-2 py-1.5 text-left transition-colors duration-[var(--dur-1)] ${
                    !ready
                      ? 'cursor-not-allowed border-line bg-card/60 text-muted opacity-60'
                      : picked === row.iid
                        ? 'cursor-pointer border-accent bg-accent/10 text-fg'
                        : 'cursor-pointer border-line bg-card text-fg2 hover:bg-hover'
                  }`}
                >
                  <span className="min-w-0 truncate">{row.name}</span>
                  <span className="shrink-0 text-xs text-muted">
                    {ready ? row.platform_label : '缺客户端档案'}
                  </span>
                </button>
              );
            })}
          </div>
        )}
        {notReady.length > 0 && (
          <p className="rounded-md border border-warn/40 bg-warn/10 px-2 py-1.5 text-xs text-fg2">
            标「缺客户端档案」的号是网页登录进来的：客户端登录态只能由客户端那次登录签发。
            在 WorkBuddy 客户端里登录一次该账号（账号中心 → 添加账号 → 改用客户端登录），
            档案会在那次流程里保存，之后本体与两个端都能切过去。
          </p>
        )}
      </div>
    </Dialog>
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
              <p>WorkBuddy 切换完成后，会把原账号名下的历史会话一并关联到新账号。</p>
            )}
          </>
        ) : (
          <>
            <p className="whitespace-pre-wrap text-fg">
              「{target?.name}」没有客户端登录态档案，因此切不过去。
            </p>
            <p className="rounded-md border border-line bg-card px-2 py-1.5">
              档案里装的是客户端那次登录签发的会话与票据，网页登录造不出来。
              在 WorkBuddy 客户端里登录一次这个账号（账号中心 → 添加账号 → 选 WorkBuddy →
              「改用客户端登录」），档案会在那次流程里保存，之后就能一键来回切。
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
          把本机已有的项目会话关联到「{target?.name}」：只改会话归属引用，
          正文一个字节都不动，可重复执行，不会产生副本。
        </p>
        <p>会先关闭 WorkBuddy，写客户端库前自动备份整份库文件，完成后重新打开客户端。</p>
      </div>
    </Dialog>
  );
}
