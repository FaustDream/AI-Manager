import type { Manifest } from '@/app/router';

// 保留同一能力入口，作品库、发行日历与采集管理共享元数据。
export const manifest: Manifest = {
  id: 'catalog',
  title: '媒体资料库',
  icon: 'catalog',
  order: 9,
  group: '效率工具',
  note: '全球综艺、动漫、漫画与影视元数据，确认发行日历与来源采集',
  fluid: true,
};
