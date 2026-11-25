// 悬停提示：Radix Tooltip 底座，样式全走令牌。
//
// 为什么换掉原生 title：原生提示延迟约 1 秒、位置不可控、会被表格的
// overflow 裁掉、多行 \n 在不同浏览器表现不一，而且完全无法样式化——
// 账号中心那种「主行结论 + 第二行原因 + 下一步」的三行说明，原生框读起来很糟。
//
// 对外只有两个东西：`Tooltip`（包任意触发元素）与 `Hint`（给纯文本/图标加提示）。
// 页面不直接碰 Radix，换底座不动页面（规范：不裸用第二套，必须包进共享层）。
import * as Tip from '@radix-ui/react-tooltip';
import { isValidElement, type ReactNode } from 'react';
import { cn } from './button';

/** 提示内容：字符串按 \n 拆行，第一行是结论，后续行是原因与下一步。 */
function Body({ text }: { text: string }) {
  const lines = text.split('\n').filter((line) => line.trim().length > 0);
  if (lines.length === 0) return null;
  const [head, ...rest] = lines;
  return (
    <>
      <span className="block font-medium text-fg">{head}</span>
      {rest.map((line, i) => (
        <span key={i} className="mt-1 block text-fg2">
          {line}
        </span>
      ))}
    </>
  );
}

export interface TooltipProps {
  /** 提示正文。空串 / undefined 时不挂提示，触发元素原样渲染。 */
  content?: string | null;
  children: ReactNode;
  side?: 'top' | 'right' | 'bottom' | 'left';
  align?: 'start' | 'center' | 'end';
  /** 宽内容（多行口径说明）放宽到 360px，默认 260px */
  wide?: boolean;
  className?: string;
}

/**
 * 给任意元素加悬停提示。触发元素必须是单个可聚焦节点（Radix 要挂 ref 与事件）。
 *
 * 延迟 220ms：比原生 1 秒快得多，又不至于鼠标扫过表格时一路闪提示。
 * 碰撞翻转 + 8px 视口边距由 Radix 负责，所以表格边缘的提示不会被裁掉。
 */
export function Tooltip({
  content,
  children,
  side = 'top',
  align = 'center',
  wide,
  className,
}: TooltipProps) {
  if (!content || !content.trim()) return <>{children}</>;
  // 禁用按钮不派发指针事件：Radix 触发器挂在按钮上时提示永远不出现。
  // 这类元素退回到 span 包裹，让「为什么不能点」仍然读得到。
  if (isValidElement(children) && (children.props as { disabled?: boolean }).disabled) {
    return (
      <Hint content={content} side={side} align={align} wide={wide} className={className} focusable={false}>
        {children}
      </Hint>
    );
  }
  return (
    <Tip.Root>
      <Tip.Trigger asChild>{children}</Tip.Trigger>
      <Tip.Portal>
        <Tip.Content
          side={side}
          align={align}
          sideOffset={6}
          collisionPadding={8}
          className={cn(
            'z-[70] rounded-md border border-line bg-card px-2.5 py-2 text-xs leading-relaxed shadow-pop',
            'data-[state=open]:animate-[fade-in_var(--dur-2)_ease-out]',
            wide ? 'max-w-[360px]' : 'max-w-[260px]',
            className,
          )}
        >
          <Body text={content} />
          <Tip.Arrow className="fill-[var(--color-card)] stroke-[var(--color-line)]" width={9} height={5} />
        </Tip.Content>
      </Tip.Portal>
    </Tip.Root>
  );
}

/**
 * 纯提示挂载点：内容本身没有交互（表格单元格、状态徽标、说明文字）时用它。
 * 渲染成 span 而不是 button——不该为了一个提示造出一个可点击控件。
 */
export function Hint({
  content,
  children,
  side,
  align,
  wide,
  className,
  focusable = true,
}: TooltipProps & {
  /** 表格单元格这类「整行已可聚焦」的场景传 false，避免凭空多出上百个 tab 停靠点。 */
  focusable?: boolean;
}) {
  if (!content || !content.trim()) return <>{children}</>;
  return (
    <Tooltip content={content} side={side} align={align} wide={wide}>
      <span tabIndex={focusable ? 0 : undefined} className={cn('outline-none', className)}>
        {children}
      </span>
    </Tooltip>
  );
}

/** 全局延迟与跳过策略。挂在 Shell 一次，所有 Tooltip 共用。 */
export function TooltipProvider({ children }: { children: ReactNode }) {
  return (
    <Tip.Provider delayDuration={220} skipDelayDuration={400} disableHoverableContent={false}>
      {children}
    </Tip.Provider>
  );
}