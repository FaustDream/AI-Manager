// 导航图标：三种布局（侧边栏 / 顶部标签 / 图标轨道）共用同一张表。
// 全部来自 lucide-react（ISC 许可，免费图标库），不与其他图标集混用风格。
import {
  Blocks,
  Clapperboard,
  LayoutDashboard,
  ListTodo,
  MessagesSquare,
  Network,
  ScrollText,
  Settings,
  Users,
} from 'lucide-react';
import type { ComponentType } from 'react';

const MAP: Record<string, ComponentType<{ size?: number; className?: string }>> = {
  overview: LayoutDashboard,
  'account-center': Users,
  sessions: MessagesSquare,
  hub: Blocks,
  'model-api': Network,
  dida: ListTodo,
  catalog: Clapperboard,
  logs: ScrollText,
  settings: Settings,
};

export function navIcon(id: string) {
  return MAP[id] ?? Users;
}