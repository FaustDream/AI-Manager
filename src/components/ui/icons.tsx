// 动作图标：**全部来自 lucide-react**（ISC 许可，免费图标库）。
//
// 这里原先内联了一套 IconPark（48 网格 / strokeWidth 4），与页面其余部分的
// lucide（24 网格 / strokeWidth 2）混用——同一个工具栏里两种笔画气质，
// 是「图标不统一」最直接的来源。现在统一到 lucide 一套网格、一套笔画。
//
// 键名（IconName）保持不变：列定义里只存名字，渲染时才取组件，
// 所以换图标库不用改任何列定义或页面。
import {
  ArrowLeftRight,
  Banknote,
  Ban,
  CalendarCheck,
  Check,
  Clock,
  Gift,
  History,
  KeyRound,
  Laptop,
  Link,
  LoaderCircle,
  NotebookPen,
  Pencil,
  Power,
  RefreshCw,
  SquarePlus,
  Trash2,
  TriangleAlert,
} from 'lucide-react';
import type { LucideProps } from 'lucide-react';

export type IconProps = {
  /** 边长（px），默认 14：和表格里 text-xs 的行高配 */
  size?: number;
} & Omit<LucideProps, 'width' | 'height' | 'size'>;

const wrap = (Glyph: typeof Check) => {
  const Wrapped = ({ size = 14, ...rest }: IconProps) => (
    <Glyph size={size} aria-hidden focusable={false} {...rest} />
  );
  return Wrapped;
};

/** 切换登录 */
export const IconSwitch = wrap(ArrowLeftRight);
/** 恢复对话 */
export const IconHistory = wrap(History);
/** 停用 / 启用 */
export const IconPower = wrap(Power);
/** 删除 */
export const IconDelete = wrap(Trash2);
/** 刷新凭据 */
export const IconRefresh = wrap(RefreshCw);
/** 已签 */
export const IconCheck = wrap(Check);
/** 未签 / 待签 */
export const IconClock = wrap(Clock);
/** 免签到 */
export const IconForbid = wrap(Ban);
/** 待查询 */
export const IconLoading = wrap(LoaderCircle);
/** 添加账号 */
export const IconAdd = wrap(SquarePlus);
/** 刷新积分 */
export const IconMoney = wrap(Banknote);
/** 刷新账号状态（本机客户端） */
export const IconLaptop = wrap(Laptop);
/** 刷新凭据 / 登录授权 */
export const IconKey = wrap(KeyRound);
/** 签到 */
export const IconCalendar = wrap(CalendarCheck);
/** 改名 */
export const IconEdit = wrap(Pencil);
/** 备注 */
export const IconNotebook = wrap(NotebookPen);
/** 重新授权 */
export const IconLink = wrap(Link);
/** 领取活动 */
export const IconGift = wrap(Gift);
export const IconWarning = wrap(TriangleAlert);

/** 图标名 → 组件。列定义里只存名字，渲染时才取组件（纯数据不碰 JSX）。 */
export const ICONS = {
  switch: IconSwitch,
  history: IconHistory,
  power: IconPower,
  delete: IconDelete,
  refresh: IconRefresh,
  check: IconCheck,
  clock: IconClock,
  forbid: IconForbid,
  loading: IconLoading,
  add: IconAdd,
  money: IconMoney,
  laptop: IconLaptop,
  key: IconKey,
  calendar: IconCalendar,
  edit: IconEdit,
  notebook: IconNotebook,
  link: IconLink,
  gift: IconGift,
  warning: IconWarning,
} as const;

export type IconName = keyof typeof ICONS;