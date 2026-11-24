// 侧边栏：品牌区 + 菜单 + 折叠开关。
//
// 两种宽度共用一套结构：240px 展开（图标 + 文字）与 56px 图标轨道
// （只留图标，说明进 title 气泡）。折叠只改宽度与文字显隐，**不换组件树**——
// 换树会让折叠那一下丢掉焦点与滚动位置。
//
// 选中态三件事一起给：淡主色底 + 左侧指示条 + 图标转主色。
// 只靠底色在四套风格里深浅不一，指示条是那条在所有风格下都读得出的锚。
import type { Capability } from '@/app/router';
import { cn } from '@/components/ui/button';
import { navIcon } from './nav-icons';
import { PanelLeftClose, PanelLeftOpen } from 'lucide-react';

export function Sidebar({
  caps,
  active,
  collapsed,
  onGo,
  onToggle,
}: {
  caps: Capability[];
  active: string;
  collapsed: boolean;
  onGo: (id: string) => void;
  onToggle: () => void;
}) {
  return (
    <aside
      className={cn(
        'relative z-20 flex shrink-0 flex-col border-r border-line bg-bg',
        'transition-[width] duration-[var(--dur-3)] ease-[var(--ease-std)]',
        collapsed ? 'w-14' : 'w-60',
      )}
    >
      {/* 品牌区：渐变方块取自当前风格的三个色相，换肤时它跟着变 */}
      <div className={cn('flex h-14 items-center gap-2.5 border-b border-line', collapsed ? 'justify-center px-2' : 'px-4')}>
        <span
          aria-hidden
          className="grid h-8 w-8 shrink-0 place-items-center rounded-md text-xs font-bold text-on-accent shadow-card"
          style={{
            backgroundImage:
              'linear-gradient(135deg, var(--color-accent), var(--color-iris) 55%, var(--color-info))',
          }}
        >
          AI
        </span>
        {!collapsed && (
          <span className="block truncate text-md leading-5 font-semibold tracking-tight">AI Manager</span>
        )}
      </div>

      <nav className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden py-3" aria-label="主导航">
        <ul className={cn('flex flex-col gap-0.5', collapsed ? 'px-2' : 'px-2.5')}>
          {caps.map((cap, index) => {
            const Icon = navIcon(cap.manifest.id);
            const on = active === cap.manifest.id;
            const at = index + 1;
            return (
              <li key={cap.manifest.id}>
                <button
                  type="button"
                  onClick={() => onGo(cap.manifest.id)}
                  aria-current={on ? 'page' : undefined}
                  // 图标轨道里没有可见文字，可读名字必须显式给（title 只是兜底）
                  aria-label={cap.manifest.title}
                  title={
                    collapsed
                      ? `${cap.manifest.title}（快捷键 ${at}）${cap.manifest.note ? ` — ${cap.manifest.note}` : ''}`
                      : `${cap.manifest.note ?? cap.manifest.title}（快捷键 ${at}）`
                  }
                  className={cn(
                    'group relative flex w-full cursor-pointer items-center gap-2.5 rounded-md py-2 text-sm',
                    'transition-colors duration-[var(--dur-1)]',
                    collapsed ? 'justify-center px-0' : 'px-2.5',
                    on
                      ? 'bg-accent-soft font-medium text-fg'
                      : 'text-fg2 hover:bg-hover hover:text-fg',
                  )}
                >
                  {on && (
                    <span
                      aria-hidden
                      className="nav-marker absolute top-1/2 left-0 h-5 w-[3px] -translate-y-1/2 rounded-r-full bg-accent"
                    />
                  )}
                  <Icon
                    size={16}
                    className={cn(
                      'shrink-0 transition-colors duration-[var(--dur-1)]',
                      on ? 'text-accent' : 'text-muted group-hover:text-fg2',
                    )}
                  />
                  {!collapsed && <span className="min-w-0 truncate">{cap.manifest.title}</span>}
                </button>
              </li>
            );
          })}
        </ul>
      </nav>

      {/* 页脚只留折叠开关：图标即可自解释，文字说明不再占位 */}
      <div className="border-t border-line px-2 py-2">
        <button
          type="button"
          onClick={onToggle}
          aria-label={collapsed ? '展开侧边栏' : '收起侧边栏'}
          title={collapsed ? '展开侧边栏' : '收起为图标轨道'}
          className={cn(
            'flex w-full cursor-pointer items-center justify-center rounded-md px-2 py-1.5 text-xs text-muted',
            'transition-colors duration-[var(--dur-1)] hover:bg-hover hover:text-fg',
          )}
        >
          {collapsed ? <PanelLeftOpen size={15} /> : <PanelLeftClose size={15} />}
        </button>
      </div>
    </aside>
  );
}