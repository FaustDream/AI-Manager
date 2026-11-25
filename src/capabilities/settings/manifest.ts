import type { Manifest } from '@/app/router';

export const manifest: Manifest = {
  id: 'settings',
  title: '设置',
  icon: 'settings',
  order: 7,
  group: '运行与配置',
  note: '外观、通知、定时任务、空间与回收站',
  // 设置页永远保留：功能开关本身就在这里，关掉就再也进不来了。
  alwaysOn: true,
};
