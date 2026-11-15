// 会话页：活跃列表。实现全部在共享的 SessionListPage（sessions/list.tsx）——
// 与「已归档会话」页共用同一套筛选 / 列表 / 批量操作 / 详情分栏。
// 本文件只剩薄壳：消费跨页预设（账号中心 / 仪表盘下钻）。
import { SessionListPage } from './list';

export default function SessionsPage() {
  return <SessionListPage mode="active" />;
}
