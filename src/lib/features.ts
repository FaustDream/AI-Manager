// 平台功能开关的前端入口：界面该不该画某个入口，由后端的「能力 × 配置」矩阵决定。
//
// 页面里不再写 `platform === 'workbuddy'` 这种判断——平台名单与可用性都在
// `capability.rs` 一张表上，用户勾选存在 `config.toml`。前端只认 `on(platform, feature)`。
//
// 取不到矩阵时一律按「开」处理：这是显示开关，一次请求失败不该把界面上的操作全藏掉。
import { useCallback } from 'react';
import { useQuery } from '@tanstack/react-query';
import { queryCmd } from './cmd';
import type { FeatureMatrix } from './types';

/** 功能键：与后端 `app::features` 的常量同名。 */
export const FEATURE_CHECKIN = 'checkin';
export const FEATURE_SWITCH = 'switch';
/** 切换 CodeBuddy IDE 的登录态（只有 WorkBuddy 国内版账号能写） */
export const FEATURE_SWITCH_IDE = 'switch_ide';
/** 切换 IntelliJ IDEA 里 CodeBuddy 插件的登录态（同上） */
export const FEATURE_SWITCH_IDEA = 'switch_idea';
export const FEATURE_RESTORE = 'restore';
export const FEATURE_TRAVEL = 'travel';

export type FeatureOn = (platform: string, feature: string) => boolean;

/**
 * 账号 → 功能表的平台键。
 *
 * WorkBuddy 国内/国际共用 `platform` 前缀，靠 iid 的 `@intl` 后缀区分——这与后端
 * `features::platform_of_iid` 同一口径，两版的能力不相同（国际版没有签到与旅行）。
 */
export function featurePlatform(platform: string, iid?: string): string {
  return platform === 'workbuddy' && iid?.endsWith('@intl') ? 'workbuddy_intl' : platform;
}

/** 是否至少有一家平台开着这个功能（取不到矩阵时按开处理，避免按钮闪掉）。 */
export function anyFeatureOn(data: FeatureMatrix | undefined, feature: string): boolean {
  if (!data) return true;
  return data.platforms.some((row) => row.cells.some((cell, index) => data.features[index]?.key === feature && cell.enabled));
}

export function usePlatformFeatures(): { data?: FeatureMatrix; on: FeatureOn } {
  const { data } = useQuery<FeatureMatrix>({
    queryKey: ['platform-features'],
    queryFn: () => queryCmd('platform_features'),
    // 配置改完会在设置页显式失效这个 key，日常不需要频繁重取
    staleTime: 30_000,
  });

  const on = useCallback<FeatureOn>(
    (platform, feature) => {
      if (!data) return true;
      const row = data.platforms.find((p) => p.key === platform);
      const index = data.features.findIndex((f) => f.key === feature);
      // 表里查不到这一格（老后端 / 新平台）时按「开」处理：藏掉入口的代价比多一个
      // 置灰按钮大得多——用户会以为功能被删了，而不是「这一格没配到」
      if (!row || index < 0) return true;
      return row.cells[index]?.enabled ?? true;
    },
    [data],
  );

  return { data, on };
}
