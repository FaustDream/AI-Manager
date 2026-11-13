import type { Manifest } from '@/app/router';

// 已归档会话：从会话页拆出来的「历史仓」，入口在 设置 → 已归档会话。
// hidden = 不占顶部导航，只能通过 aim:navigate 打开。
export const manifest: Manifest = {
  id: 'sessions-archive',
  title: '已归档会话',
  icon: 'sessions',
  order: 7,
  hidden: true,
};
