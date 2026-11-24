// 前端测试：只跑纯函数（src/lib），不需要 jsdom。
// TZ 固定成 Asia/Shanghai：fmtTime 的契约是「按本地时区渲染」，
// 只有固定时区，+08:00 的断言才能在 UTC 的 CI runner 上同样成立。
import { defineConfig } from 'vitest/config';

// 仓库没有装 @types/node（前端只跑纯函数测试）：这里只声明本文件用到的 Node 全局，
// 避免为一个配置文件拖进整套 Node 类型、把 process 暴露到浏览器代码的类型作用域里。
declare const process: {
  env: { TZ?: string };
  cwd(): string;
};

process.env.TZ = 'Asia/Shanghai';

export default defineConfig({
  // 与 vite.config.ts / tsconfig 的 paths 保持一致：'@' → src
  resolve: {
    alias: { '@': `${process.cwd()}/src` },
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    env: { TZ: 'Asia/Shanghai' },
  },
});
