import type { Manifest } from '@/app/router';

// order 0：仪表盘是登录后的第一屏，先看结论，再看具体账号。
export const manifest: Manifest = { id: 'overview', title: '仪表盘', icon: 'overview', order: 0 };
