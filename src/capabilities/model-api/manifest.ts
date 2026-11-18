import type { Manifest } from '@/app/router';

// 接口反代：原本挂在 设置 → 常规 的「豆包模型接口」。
// 它是一条常驻服务（监听端口、鉴权、客户端接入），不是一次性偏好设置，
// 所以单独成一级标签页，设置页只保留纯配置项。
export const manifest: Manifest = { id: 'model-api', title: '接口反代', icon: 'model-api', order: 5 };
