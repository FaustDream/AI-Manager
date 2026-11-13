// 异常区的触发规则：同一批错误只自动展开一次，只有比「提示水位」更新的错误才重新展开。
//
// 早期这里是「异常弹窗」的触发逻辑，弹窗改成页面内的可折叠区块后规则没变——
// 遮不遮住页面是表现形式，判重逻辑该不该重复提示是另一件事。
//
// 抽成纯函数是为了能单测——这条规则写错，要么旧错误每次进页都展开，要么新错误永远不展开；
// 两种症状在界面上都只表现为「它怎么又弹了 / 怎么没弹」，靠手点很难复现。
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
