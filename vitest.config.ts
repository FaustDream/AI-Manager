// 前端测试：只跑纯函数（src/lib），不需要 jsdom。
// TZ 固定成 Asia/Shanghai：fmtTime 的契约是「按本地时区渲染」，
// 只有固定时区，+08:00 的断言才能在 UTC 的 CI runner 上同样成立。
import { defineConfig } from 'vitest/config';

process.env.TZ = 'Asia/Shanghai';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    env: { TZ: 'Asia/Shanghai' },
  },
});
