// 外观：视觉风格 / 导航布局 / 界面字号。
//
// 三件事都是「所见即所得」——选项自带缩略预览，点下去立刻生效，不给「保存」按钮：
// 外观偏好没有「改坏了要回滚」的风险，多一次确认只是多一次点击。
//
// 选项用原生 radio（视觉隐藏）而不是按钮组：方向键切换、读屏报「第几项/共几项」
// 全部由浏览器负责，比自己实现一套 roving tabindex 更可靠。
import { useState, type ReactNode } from 'react';
import { Check, Monitor } from 'lucide-react';
import { cn } from '@/components/ui/button';
import { Section } from '@/components/ui/card';
import { applyFont, saveTheme, THEME_STYLES, useThemeChoice } from '@/lib/theme';
import {
  loadFontChoice,
  NAV_KEY,
  NAV_LAYOUTS,
  saveFontChoice,
  usePersistedChoice,
  type FontChoice,
  type NavLayout,
} from '@/lib/ui-state';

const NAV_META: Record<NavLayout, { name: string; note: string }> = {
  sidebar: { name: '侧边栏', note: '240px 分组菜单 + 窄顶栏，页面多时最好找' },
  topbar: { name: '顶部标签', note: '一屏全给内容，窗口矮时最省纵向空间' },
  rail: { name: '图标轨道', note: '56px 常驻轨道，可展开；表格要横向空间时用' },
};

export function AppearancePane() {
  const theme = useThemeChoice();
  const [layout, setLayout] = usePersistedChoice<NavLayout>(NAV_KEY, 'sidebar', NAV_LAYOUTS);
  const [font, setFont] = useState<FontChoice>(loadFontChoice);

  return (
    <Section
      title="外观"
      note="换肤只换令牌值，组件与页面结构不动"
      actions={<span className="text-xs text-muted">改动立即生效</span>}
    >
      <div className="flex flex-col gap-6">
        <fieldset>
          <legend className="mb-2 text-xs font-medium text-fg2">视觉风格</legend>
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            <Choice
              group="aim-style"
              checked={theme === ''}
              onPick={() => void saveTheme('')}
              label="跟随系统"
              note="浅色用简约商务，深色用暗色科技"
              preview={<SystemPreview />}
            />
            {THEME_STYLES.map((style) => (
              <Choice
                key={style.key}
                group="aim-style"
                checked={theme === style.key}
                onPick={() => void saveTheme(style.key)}
                label={style.name}
                note={style.note}
                preview={<StylePreview swatch={style.swatch} />}
              />
            ))}
          </div>
        </fieldset>

        <fieldset className="border-t border-line pt-5">
          <legend className="mb-2 text-xs font-medium text-fg2">导航布局</legend>
          <div className="grid gap-3 sm:grid-cols-3">
            {NAV_LAYOUTS.map((key) => (
              <Choice
                key={key}
                group="aim-nav"
                checked={layout === key}
                onPick={() => setLayout(key)}
                label={NAV_META[key].name}
                note={NAV_META[key].note}
                preview={<NavPreview layout={key} />}
              />
            ))}
          </div>
        </fieldset>

        <fieldset className="border-t border-line pt-5">
          <legend className="mb-2 text-xs font-medium text-fg2">界面字号</legend>
          <div className="flex flex-wrap items-center gap-3">
            {(['standard', 'large'] as const).map((key) => (
              <label
                key={key}
                className={cn(
                  'flex cursor-pointer items-center gap-2 rounded-md border px-3 py-1.5 text-sm transition-colors duration-[var(--dur-1)]',
                  font === key
                    ? 'border-accent bg-accent-soft text-accent-strong'
                    : 'border-line text-fg2 hover:bg-hover',
                )}
              >
                <input
                  type="radio"
                  name="aim-font"
                  className="h-3.5 w-3.5 accent-[var(--color-accent)]"
                  checked={font === key}
                  onChange={() => {
                    setFont(key);
                    applyFont(key);
                    saveFontChoice(key);
                  }}
                />
                {key === 'standard' ? '标准' : '大（每档 +1px）'}
              </label>
            ))}
            <span className="text-xs text-muted">
              正文、表格与说明文字一起放大；只影响本机显示，不写进运行配置
            </span>
          </div>
        </fieldset>
      </div>
    </Section>
  );
}

/** 选项卡：隐藏 radio（`group` 是分组名）+ 缩略预览 + 名称说明。 */
function Choice({
  group,
  label,
  note,
  checked,
  onPick,
  preview,
}: {
  group: string;
  label: string;
  note: string;
  checked: boolean;
  onPick: () => void;
  preview: ReactNode;
}) {
  return (
    <label className="group cursor-pointer">
      <input
        type="radio"
        name={group}
        className="peer sr-only"
        checked={checked}
        onChange={onPick}
        aria-label={label}
      />
      <span
        className={cn(
          'block rounded-lg border bg-bg p-2.5 transition-[border-color,box-shadow,background-color] duration-[var(--dur-2)]',
          'group-hover:border-line-strong group-hover:shadow-card',
          'peer-focus-visible:ring-2 peer-focus-visible:ring-accent/45 peer-focus-visible:ring-offset-2 peer-focus-visible:ring-offset-bg',
          checked ? 'border-accent shadow-card' : 'border-line',
        )}
      >
        <span className="relative block overflow-hidden rounded-md border border-line">
          {preview}
          <span
            className={cn(
              'absolute top-1 right-1 grid h-4 w-4 place-items-center rounded-full bg-accent text-on-accent transition-opacity duration-[var(--dur-2)]',
              checked ? 'opacity-100' : 'opacity-0',
            )}
          >
            <Check size={11} strokeWidth={3} />
          </span>
        </span>
        <span className="mt-2 flex items-center gap-1.5">
          <span className={cn('text-sm font-medium', checked ? 'text-accent-strong' : 'text-fg')}>
            {label}
          </span>
        </span>
        <span className="mt-0.5 block text-xs leading-4 text-muted">{note}</span>
      </span>
    </label>
  );
}

/** 风格缩略图：画布 + 侧栏 + 卡片 + 两枚强调色，全部用该风格的真实令牌值。 */
function StylePreview({ swatch }: { swatch: readonly [string, string, string, string] }) {
  const [canvas, surface, accent, second] = swatch;
  return (
    <span className="flex h-[68px] w-full" style={{ background: canvas }}>
      <span className="w-[22%] border-r" style={{ background: surface, borderColor: canvas }}>
        <span className="mt-2 ml-1.5 block h-1.5 w-1.5 rounded-sm" style={{ background: accent }} />
        {[0, 1, 2].map((i) => (
          <span
            key={i}
            className="mx-1.5 mt-1.5 block h-1 rounded-full"
            style={{ background: i === 0 ? accent : canvas }}
          />
        ))}
      </span>
      <span className="flex-1 p-2">
        <span className="block h-1.5 w-1/3 rounded-full" style={{ background: accent }} />
        <span className="mt-2 flex gap-1.5">
          <span
            className="h-6 flex-1 rounded-sm border"
            style={{ background: surface, borderColor: canvas }}
          />
          <span
            className="h-6 flex-1 rounded-sm border"
            style={{ background: surface, borderColor: canvas }}
          />
        </span>
        <span className="mt-1.5 flex items-center gap-1">
          <span className="h-1.5 w-8 rounded-full" style={{ background: accent }} />
          <span className="h-1.5 w-5 rounded-full" style={{ background: second }} />
        </span>
      </span>
    </span>
  );
}

/** 跟随系统：一张图斜切成浅/深两半，说明它不是第五套配色而是「按 OS 选」。 */
function SystemPreview() {
  const light = THEME_STYLES[0].swatch;
  const dark = THEME_STYLES[1].swatch;
  return (
    <span className="relative block h-[68px] w-full">
      <span className="absolute inset-0">
        <StylePreview swatch={light} />
      </span>
      <span className="absolute inset-0" style={{ clipPath: 'polygon(100% 0, 100% 100%, 38% 100%)' }}>
        <StylePreview swatch={dark} />
      </span>
      <span
        className="absolute inset-y-0 left-[38%] w-px -rotate-[24deg] origin-bottom"
        style={{ background: 'rgb(148 163 184 / .55)' }}
      />
      <span className="absolute bottom-1 left-1 grid h-4 w-4 place-items-center rounded-full bg-bg/85 text-fg2">
        <Monitor size={10} />
      </span>
    </span>
  );
}

/** 导航缩略图：三种布局的线框，用当前风格令牌画——切风格时预览跟着换。 */
function NavPreview({ layout }: { layout: NavLayout }) {
  const bar = <span className="block h-2.5 w-full border-b border-line bg-sunken" />;
  const lines = (n: number, w = 'w-2/3') =>
    Array.from({ length: n }, (_, i) => (
      <span key={i} className={cn('mx-1.5 mt-1.5 block h-1 rounded-full bg-line-strong', w)} />
    ));
  return (
    <span className="flex h-[68px] w-full flex-col bg-bg">
      {layout === 'topbar' ? (
        <>
          <span className="flex h-3.5 items-center gap-1 border-b border-line bg-sunken px-1.5">
            <span className="h-1.5 w-1.5 rounded-sm bg-accent" />
            {[0, 1, 2, 3].map((i) => (
              <span
                key={i}
                className={cn('h-1 w-4 rounded-full', i === 0 ? 'bg-accent' : 'bg-line-strong')}
              />
            ))}
          </span>
          <span className="flex-1 p-1.5">
            <span className="block h-1.5 w-1/3 rounded-full bg-fg2/40" />
            <span className="mt-1.5 block h-8 rounded-sm border border-line bg-sunken" />
          </span>
        </>
      ) : (
        <span className="flex h-full">
          <span
            className={cn(
              'border-r border-line bg-sunken pt-1',
              layout === 'rail' ? 'w-[16%]' : 'w-[32%]',
            )}
          >
            {layout === 'rail'
              ? [0, 1, 2, 3].map((i) => (
                  <span
                    key={i}
                    className={cn(
                      'mx-auto mt-1.5 block h-1.5 w-1.5 rounded-sm',
                      i === 0 ? 'bg-accent' : 'bg-line-strong',
                    )}
                  />
                ))
              : lines(4)}
          </span>
          <span className="flex min-w-0 flex-1 flex-col">
            {bar}
            <span className="flex-1 p-1.5">
              <span className="block h-1.5 w-1/3 rounded-full bg-fg2/40" />
              <span className="mt-1.5 block h-6 rounded-sm border border-line bg-sunken" />
            </span>
          </span>
        </span>
      )}
    </span>
  );
}