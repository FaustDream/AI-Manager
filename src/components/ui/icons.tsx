// IconPark 图标（单色 / currentColor 跟随文字颜色）。
//
// 来源：https://iconpark.oceanengine.com/ · Apache-2.0。
// **不引 `@icon-park/react`**：CSP 是 `'self'`，内联 SVG 没有额外请求；为九个图标
// 多背一个 npm 依赖不划算，而且这里只用到 mono 一种主题。
//
// 用法：`<IconSwitch />`，尺寸走 `size`（默认 14，跟 text-xs 行高配），
// 颜色由父元素的 `color` 决定，`className` 可以直接上 Tailwind 的 text-*。
import type { ReactNode, SVGProps } from 'react';

export type IconProps = {
  /** 边长（px），默认 14：和表格里 text-xs 的行高配 */
  size?: number;
} & Omit<SVGProps<SVGSVGElement>, 'width' | 'height'>;

function Icon({ size = 14, children, ...rest }: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 48 48"
      fill="none"
      aria-hidden
      focusable={false}
      {...rest}
    >
      {children}
    </svg>
  );
}

/** 切换登录：左右双向箭头 */
export const IconSwitch = (p: IconProps) => (
  <Icon {...p}>
    <path d="M42 19H5.99998" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M30 7L42 19" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M6.79897 29H42.799" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M6.79895 29L18.799 41" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 恢复对话：带指针的历史记录 */
export const IconHistory = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5.81836 6.72729V14H13.0911" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path
      d="M4 24C4 35.0457 12.9543 44 24 44V44C35.0457 44 44 35.0457 44 24C44 12.9543 35.0457 4 24 4C16.598 4 10.1351 8.02111 6.67677 13.9981"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
    <path d="M24.005 12L24.0038 24.0088L32.4832 32.4882" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 停用 / 启用：电源开关 */
export const IconPower = (p: IconProps) => (
  <Icon {...p}>
    <path
      d="M14.5 8C13.8406 8.37652 13.2062 8.79103 12.6 9.24051C11.5625 10.0097 10.6074 10.8814 9.75 11.8402C6.79377 15.1463 5 19.4891 5 24.2455C5 34.6033 13.5066 43 24 43C34.4934 43 43 34.6033 43 24.2455C43 19.4891 41.2062 15.1463 38.25 11.8402C37.3926 10.8814 36.4375 10.0097 35.4 9.24051C34.7938 8.79103 34.1594 8.37652 33.5 8"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
    <path d="M24 4V24" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 删除：垃圾桶 */
export const IconDelete = (p: IconProps) => (
  <Icon {...p}>
    <path d="M9 10V44H39V10H9Z" fill="none" stroke="currentColor" strokeWidth={4} strokeLinejoin="round" />
    <path d="M20 20V33" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M28 20V33" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M4 10H44" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M16 10L19.289 4H28.7771L32 10H16Z" fill="none" stroke="currentColor" strokeWidth={4} strokeLinejoin="round" />
  </Icon>
);

/** 刷新凭据：循环箭头 */
export const IconRefresh = (p: IconProps) => (
  <Icon {...p}>
    <path d="M42 8V24" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M6 24L6 40" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path
      d="M42 24C42 14.0589 33.9411 6 24 6C18.9145 6 14.3216 8.10896 11.0481 11.5M6 24C6 33.9411 14.0589 42 24 42C28.8556 42 33.2622 40.0774 36.5 36.9519"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </Icon>
);

/** 已签：对勾 */
export const IconCheck = (p: IconProps) => (
  <Icon {...p}>
    <path d="M43 11L16.875 37L5 25.1818" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 未签 / 待签：钟表 */
export const IconClock = (p: IconProps) => (
  <Icon {...p}>
    <path
      d="M24 44C35.0457 44 44 35.0457 44 24C44 12.9543 35.0457 4 24 4C12.9543 4 4 12.9543 4 24C4 35.0457 12.9543 44 24 44Z"
      fill="none"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinejoin="round"
    />
    <path d="M24.0084 12.0001L24.0072 24.0089L32.4866 32.4883" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 免签到：禁止 */
export const IconForbid = (p: IconProps) => (
  <Icon {...p}>
    <path
      fillRule="evenodd"
      clipRule="evenodd"
      d="M24 44C35.0457 44 44 35.0457 44 24C44 12.9543 35.0457 4 24 4C12.9543 4 4 12.9543 4 24C4 35.0457 12.9543 44 24 44Z"
      fill="none"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
    <path d="M15 15L33 33" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 待查询：转圈 */
export const IconLoading = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 24C4 35.0457 12.9543 44 24 44V44C35.0457 44 44 35.0457 44 24C44 12.9543 35.0457 4 24 4" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path
      d="M36 24C36 17.3726 30.6274 12 24 12C17.3726 12 12 17.3726 12 24C12 30.6274 17.3726 36 24 36V36"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </Icon>
);

// ---- 工具栏 / 详情页的动作用图标（按钮文字过长，一律图标 + title 气泡）----

/** 添加账号 */
export const IconAdd = (p: IconProps) => (
  <Icon {...p}>
    <rect x="6" y="6" width="36" height="36" rx="3" fill="none" stroke="currentColor" strokeWidth={4} strokeLinejoin="round" />
    <path d="M24 16V32" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M16 24L32 24" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 刷新积分：钞票 */
export const IconMoney = (p: IconProps) => (
  <Icon {...p}>
    <path
      d="M39 7H9C6.23858 7 4 9.23858 4 12V36C4 38.7614 6.23858 41 9 41H39C41.7614 41 44 38.7614 44 36V12C44 9.23858 41.7614 7 39 7Z"
      fill="none"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinejoin="round"
    />
    <path d="M18 15L24 21L30 15" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M17 23H31" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M17 29H31" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M24 23V34" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 刷新账号状态：本机（客户端） */
export const IconLaptop = (p: IconProps) => (
  <Icon {...p}>
    <rect x="8" y="8" width="32" height="20" fill="none" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M8 28L4 41H44L40 28" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M19.9 35H28.1L30 41H18L19.9 35Z" fill="none" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 刷新凭据 / 登录授权：钥匙 */
export const IconKey = (p: IconProps) => (
  <Icon {...p}>
    <path
      d="M22.8682 24.2982C25.4105 26.7935 26.4138 30.4526 25.4971 33.8863C24.5805 37.32 21.8844 40.0019 18.4325 40.9137C14.9806 41.8256 11.3022 40.8276 8.79375 38.2986C5.02208 34.4141 5.07602 28.2394 8.91499 24.4206C12.754 20.6019 18.9613 20.5482 22.8664 24.3L22.8682 24.2982Z"
      fill="none"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinejoin="round"
    />
    <path d="M23 24L40 7" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M30.3052 16.9001L35.7337 22.3001L42.0671 16.0001L36.6385 10.6001L30.3052 16.9001Z" fill="none" stroke="currentColor" strokeWidth={4} strokeLinejoin="round" />
  </Icon>
);

/** 签到：日历 */
export const IconCalendar = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5 19H43V40C43 41.1046 42.1046 42 41 42H7C5.89543 42 5 41.1046 5 40V19Z" fill="none" stroke="currentColor" strokeWidth={4} strokeLinejoin="round" />
    <path d="M5 9C5 7.89543 5.89543 7 7 7H41C42.1046 7 43 7.89543 43 9V19H5V9Z" stroke="currentColor" strokeWidth={4} strokeLinejoin="round" />
    <path d="M16 4V12" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M32 4V12" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M28 34H34" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M14 34H20" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M28 26H34" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M14 26H20" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 改名：铅笔 */
export const IconEdit = (p: IconProps) => (
  <Icon {...p}>
    <path d="M7 42H43" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M11 26.7199V34H18.3172L39 13.3081L31.6951 6L11 26.7199Z" fill="none" stroke="currentColor" strokeWidth={4} strokeLinejoin="round" />
  </Icon>
);

/** 备注：笔记本 */
export const IconNotebook = (p: IconProps) => (
  <Icon {...p}>
    <path
      d="M10 6C10 4.89543 10.8954 4 12 4H40C41.1046 4 42 4.89543 42 6V42C42 43.1046 41.1046 44 40 44H12C10.8954 44 10 43.1046 10 42V6Z"
      fill="none"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinejoin="round"
    />
    <path d="M34 6V42" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M6 14H14" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M6 24H14" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M6 34H14" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M27 4H39" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M27 44H39" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

/** 重新授权：链接 */
export const IconLink = (p: IconProps) => (
  <Icon {...p}>
    <path
      d="M24.7073 9.56521L9.85801 24.4145C6.34329 27.9292 6.34329 33.6277 9.85801 37.1424V37.1424C13.3727 40.6571 19.0712 40.6571 22.5859 37.1424L40.2636 19.4647C42.6067 17.1216 42.6067 13.3226 40.2636 10.9794V10.9794C37.9205 8.63628 34.1215 8.63628 31.7783 10.9794L14.1007 28.6571C12.9291 29.8287 12.9291 31.7282 14.1007 32.8997V32.8997C15.2722 34.0713 17.1717 34.0713 18.3433 32.8997L33.1925 18.0505"
      stroke="currentColor"
      strokeWidth={4}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </Icon>
);

/** 领取活动：礼盒 */
export const IconGift = (p: IconProps) => (
  <Icon {...p}>
    <path d="M41 44V20H7V44H41Z" fill="none" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M24 44V20" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <path d="M41 44H7" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
    <rect x="4" y="12" width="40" height="8" fill="none" stroke="currentColor" strokeWidth={4} strokeLinejoin="round" />
    <path d="M16 4L24 12L32 4" stroke="currentColor" strokeWidth={4} strokeLinecap="round" strokeLinejoin="round" />
  </Icon>
);

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
} as const;

export type IconName = keyof typeof ICONS;
