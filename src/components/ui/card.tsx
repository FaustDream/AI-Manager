// 内容容器三件套：页面标题 / 区块卡片 / 裸卡片。
//
// 卡片外观只有这一个入口（圆角、边框、底色、阴影都取自令牌），页面不各写一套。
// 分工：
//   PageHeader  页面级标题 + 一句话说明 + 页面级动作（每页恰好一个）
//   Section     带标题栏的区块卡片（叙事页的主力：图表、配置组、说明块）
//   Card        无标题栏的卡片（KPI 瓦片、提示块、卡中卡）
//
// 表格页不用 Section 包表格：表格自身的边框就是数据边界，再套一层卡片
// 会变成「卡片里的卡片」，白白吃掉两圈留白。
import type { HTMLAttributes, ReactNode } from 'react';
import { cn } from './button';

export function PageHeader({
  title,
  note,
  actions,
  className,
}: {
  title: ReactNode;
  /** 一句话说清「这页管什么」。不写功能清单，写用户来这儿要干的事。 */
  note?: ReactNode;
  /** 页面级动作：右侧成组，窄屏自动换行到标题下方 */
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn('flex flex-wrap items-start gap-x-6 gap-y-3', className)}>
      <div className="min-w-0 flex-1">
        <h1 className="text-2xl font-semibold tracking-tight text-fg">{title}</h1>
        {note && <p className="mt-1 max-w-[72ch] text-sm text-muted">{note}</p>}
      </div>
      {actions && <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}

export function Card({
  className,
  hover,
  ...props
}: HTMLAttributes<HTMLDivElement> & { hover?: boolean }) {
  return (
    <div
      className={cn(
        'rounded-lg border border-line bg-card p-4 shadow-card',
        hover &&
          'transition-[box-shadow,border-color] duration-[var(--dur-2)] hover:border-line-strong hover:shadow-card-hover',
        className,
      )}
      {...props}
    />
  );
}

export function Section({
  title,
  note,
  actions,
  children,
  className,
  bodyClassName,
  flush,
}: {
  title: ReactNode;
  /** 标题右侧的口径说明（区间、单位、更新时间），小字不抢标题 */
  note?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  bodyClassName?: string;
  /** 内容自带边界（表格、图表、列表）时去掉内边距，避免双圈留白 */
  flush?: boolean;
}) {
  return (
    <section
      className={cn(
        'min-w-0 overflow-hidden rounded-lg border border-line bg-card shadow-card',
        className,
      )}
    >
      <header className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-line px-5 py-3">
        <h2 className="text-lg font-semibold tracking-tight text-fg">{title}</h2>
        {note && <span className="text-xs text-muted">{note}</span>}
        <span className="flex-1" />
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </header>
      <div className={cn(flush ? '' : 'p-5', bodyClassName)}>{children}</div>
    </section>
  );
}