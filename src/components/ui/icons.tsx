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
} as const;

export type IconName = keyof typeof ICONS;
