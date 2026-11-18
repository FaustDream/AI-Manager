// 异常提示的触发规则：同一批异常只提示一次，只有比「提示水位」更新的异常才再提示。
//
// 异常展示从可折叠区块改成了独立视图 + 顶部提示条，判重逻辑不变——
// 遮不遮住页面是表现形式，该不该重复提示是另一件事。
//
// 抽成纯函数是为了能单测——这条规则写错，要么旧异常每次进页都提示，要么新异常永远不提示；
// 两种症状在界面上都只表现为「它怎么又提示了 / 怎么没提示」，靠手点很难复现。
import type { LogLine } from '@/lib/types';

/** 提示水位存在 kv 里的键：值是上次展开时最新一条错误的时间戳（毫秒）。 */
export const ALERT_SEEN_KEY = 'ui.logs.alerts-seen-at';

/** 一批错误/警告里最新的时间戳（毫秒）；0 = 没有可用时间。 */
export function newestAlertAt(rows: LogLine[]): number {
  let newest = 0;
  for (const line of rows) {
    const at = Date.parse(line.ts);
    if (Number.isFinite(at) && at > newest) newest = at;
  }
  return newest;
}

/** 该不该弹：窗口里有错误，且存在比水位更新的那一条。 */
export function shouldAlert(rows: LogLine[], seenAt: number): boolean {
  return newestAlertAt(rows) > seenAt;
}
