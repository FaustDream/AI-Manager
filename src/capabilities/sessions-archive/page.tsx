// 已归档会话（独立页）：会话的「历史仓」。实现全部在共享的 SessionListPage
// （sessions/list.tsx），本文件只剩薄壳。入口在设置页与会话页头部的「已归档（N）」。
import { SessionListPage } from '../sessions/list';

export default function ArchivedSessionsPage() {
  return <SessionListPage mode="archived" />;
}
