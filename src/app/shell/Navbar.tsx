// 顶部导航栏。三种布局共用同一套右侧动作区，左侧按布局换内容：
//   sidebar / rail  折叠开关 +（收起时）当前页标签 —— 展开时标题由侧栏给出，不重复
//   topbar          品牌 + 横向标签（标签自己横向滚动，按钮不压缩）
//
// 搜索做成「看起来像输入框的按钮」：真正的搜索在命令面板里（页面/动作/会话三类），
// 这里给的是它的显性入口 —— Ctrl+K 以前是隐藏能力，界面上没有任何提示。
import type { ReactNode } from 'react';
import { ArrowLeft, PanelLeft, Search } from 'lucide-react';
import { Button, cn } from '@/components/ui/button';
import { Hint, Tooltip } from '@/components/ui/tooltip';
import { QuitButton } from './QuitButton';
import { ThemeMenu } from './ThemeMenu';
import { navIcon } from './nav-icons';
import type { NavLayout } from '@/lib/ui-state';

export function Navbar({
  layout,
  activeId,
  activeTitle,
  collapsed,
  onToggleNav,
  tabs,
  backTitle,
  onBack,
  onPalette,
  progress,
}: {
  layout: NavLayout;
  activeId: string;
  activeTitle: string;
  collapsed: boolean;
  onToggleNav: () => void;
  /** topbar 布局的横向标签条；其余布局不传 */
  tabs?: ReactNode;
  backTitle?: string | null;
  onBack: () => void;
  onPalette: () => void;
  /** 切页进度线是否可见 */
  progress: boolean;
}) {
  const ActiveIcon = navIcon(activeId);
  const topbar = layout === 'topbar';

  return (
    <header
      className={cn(
        'relative z-30 flex h-14 shrink-0 items-center gap-2 border-b border-line bg-bg px-3',
        !topbar && 'px-4',
      )}
    >
      {!topbar && (
        <Hint content={collapsed ? '展开侧边栏' : '收起侧边栏'} focusable={false}>
          <Button size="sm" variant="ghost" onClick={onToggleNav} aria-label={collapsed ? '展开侧边栏' : '收起侧边栏'}>
            <PanelLeft size={15} />
          </Button>
        </Hint>
      )}
      {!topbar && collapsed && (
        <span className="flex min-w-0 items-center gap-1.5 text-sm font-medium text-fg2">
          <ActiveIcon size={15} className="shrink-0 text-accent" />
          <span className="truncate">{activeTitle}</span>
        </span>
      )}

      {topbar && (
        <>
          <span className="flex shrink-0 items-center gap-2 pr-1">
            <span
              aria-hidden
              className="grid h-7 w-7 place-items-center rounded-md text-xs font-bold text-on-accent"
              style={{
                backgroundImage:
                  'linear-gradient(135deg, var(--color-accent), var(--color-iris) 55%, var(--color-info))',
              }}
            >
              AI
            </span>
            <span className="hidden text-md font-semibold tracking-tight lg:inline">AI Manager</span>
          </span>
          {tabs}
        </>
      )}

      <span className="flex-1" />

      <Tooltip content="搜索页面 / 动作 / 会话（Ctrl+K）">
        <button
          type="button"
          onClick={onPalette}
          aria-label="打开搜索面板"
          className={cn(
            'hidden h-8 shrink-0 cursor-pointer items-center gap-2 rounded-md border border-line bg-sunken px-2.5 text-sm text-muted',
            'transition-colors duration-[var(--dur-1)] hover:border-line-strong hover:text-fg2 md:flex',
            topbar ? 'w-52 lg:w-64' : 'w-56 lg:w-72',
          )}
        >
          <Search size={14} className="shrink-0" />
          <span className="min-w-0 flex-1 truncate text-left">搜索页面 / 动作 / 会话</span>
          <kbd className="shrink-0 rounded-sm border border-line bg-bg px-1 text-xs leading-4">Ctrl K</kbd>
        </button>
      </Tooltip>
      {/* 窄窗口下搜索收成图标按钮，入口不消失 */}
      <Hint content="搜索（Ctrl+K）" focusable={false}>
        <Button size="sm" variant="ghost" className="md:hidden" onClick={onPalette} aria-label="打开搜索面板">
          <Search size={15} />
        </Button>
      </Hint>

      {backTitle && (
        <Hint content={`返回${backTitle}`} focusable={false}>
          <Button size="sm" variant="ghost" className="group" aria-label="返回上一页" onClick={onBack}>
            <ArrowLeft size={14} className="transition-transform duration-[var(--dur-1)] group-hover:-translate-x-0.5" />
            <span className="hidden lg:inline">返回</span>
          </Button>
        </Hint>
      )}

      <ThemeMenu />
      <QuitButton />

      {/* 切页进度线：贴 header 底边，不占布局、不挡内容。轨道常驻极淡，指示块循环穿过 */}
      <span
        aria-hidden
        className={cn(
          'pointer-events-none absolute inset-x-0 bottom-0 h-0.5 overflow-hidden transition-opacity duration-[var(--dur-1)]',
          progress ? 'opacity-100' : 'opacity-0',
        )}
      >
        <span className="indeterminate-bar block h-full w-[30%] bg-accent/60" />
      </span>
    </header>
  );
}