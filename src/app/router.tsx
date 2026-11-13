// 能力清单（§4.1）：扫 capabilities/*/manifest.ts 生成导航与路由。
// 新增功能 = 新建目录放 manifest.ts + page.tsx，核心代码零改动。
//
// 页面一律懒加载（每个能力一个 chunk），但**不是等用户点开才去拉**：
// 首屏渲染完会在空闲时间把其余页面 chunk 全部预热（`Capability.preload`）。
// 之前只在点标签时才 import，弱一点的机器上要等模块图加载完，看起来就是
// 「切过去一片空白、过几秒才出内容」——预热 + 过渡期保留旧页面把这段等掉了。
import { lazy } from 'react';
import type { ComponentType } from 'react';

export interface Manifest {
  id: string;
  title: string;
  icon: string;
  order: number;
  /// 不显示在顶部导航与命令面板；只能通过 `aim:navigate` 打开（如设置页里的入口）。
  /// 归档会话这类「低频、从别处进入」的能力用它，避免占一个一级标签。
  hidden?: boolean;
}

export interface Capability {
  manifest: Manifest;
  page: ComponentType;
  /// 把页面 chunk 拉下来（已加载则立即 resolve）。切页前先 await 它，
  /// 界面就不用靠 Suspense 的 fallback 去兜「还没下载完」这段时间。
  load: () => Promise<{ default: ComponentType }>;
}

const manifests = import.meta.glob('../capabilities/*/manifest.ts', { eager: true }) as Record<
  string,
  { manifest: Manifest }
>;
const pages = import.meta.glob('../capabilities/*/page.tsx') as Record<
  string,
  () => Promise<{ default: ComponentType }>
>;

/// 已发出（或已完成）的加载：`lazy` 与 `preload` 共用同一份 Promise，
/// 避免预热和真正渲染时各发一次请求。
const inflight = new Map<string, Promise<{ default: ComponentType }>>();

function load(dir: string, loader: () => Promise<{ default: ComponentType }>) {
  const cached = inflight.get(dir);
  if (cached) return cached;
  const promise = loader();
  inflight.set(dir, promise);
  // 失败要能重试：把失败的 Promise 留着，用户点重试也永远拿到同一次失败
  promise.catch(() => inflight.delete(dir));
  return promise;
}

export function capabilities(): Capability[] {
  const out: Capability[] = [];
  for (const [path, mod] of Object.entries(manifests)) {
    const dir = path.split('/')[2];
    const pagePath = `../capabilities/${dir}/page.tsx`;
    const loader = pages[pagePath];
    if (!loader) continue;
    out.push({
      manifest: mod.manifest,
      page: lazy(() => load(dir, loader)),
      load: () => load(dir, loader),
    });
  }
  return out.sort((a, b) => a.manifest.order - b.manifest.order);
}
