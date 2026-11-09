import type { Manifest } from '@/app/router';

// order 0：仪表盘是登录后的第一屏，账号结论 → 积分收支 → 用量统计一页看完。
export const manifest: Manifest = { id: 'overview', title: '仪表盘', icon: 'overview', order: 0 };
