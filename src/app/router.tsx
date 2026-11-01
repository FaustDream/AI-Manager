// 能力清单（§4.1）：扫 capabilities/*/manifest.ts 生成导航与路由。
// 新增功能 = 新建目录放 manifest.ts + page.tsx，核心代码零改动。
import { lazy } from 'react';
import type { ComponentType } from 'react';

export interface Manifest {
  id: string;
  title: string;
  icon: string;
  order: number;
}

export interface Capability {
  manifest: Manifest;
  page: ComponentType;
}

const manifests = import.meta.glob('../capabilities/*/manifest.ts', { eager: true }) as Record<
  string,
  { manifest: Manifest }
>;
const pages = import.meta.glob('../capabilities/*/page.tsx') as Record<
  string,
  () => Promise<{ default: ComponentType }>
>;

export function capabilities(): Capability[] {
  const out: Capability[] = [];
  for (const [path, mod] of Object.entries(manifests)) {
    const dir = path.split('/')[2];
    const pagePath = `../capabilities/${dir}/page.tsx`;
    const loader = pages[pagePath];
    if (!loader) continue;
    out.push({
      manifest: mod.manifest,
      page: lazy(loader),
    });
  }
  return out.sort((a, b) => a.manifest.order - b.manifest.order);
}
