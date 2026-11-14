import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import path from 'node:path';

// Tauri 前端。产物落 dist/，tauri.conf.json 的 frontendDist 指向 ../dist。
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@': path.resolve(__dirname, 'src') },
  },
  clearScreen: false,
  server: {
    // 5173 是各路前端工具的默认端口（实测与 Reflow 的 electron-vite 撞过），
    // 本项目用专属端口 + strictPort：被占用就报错，绝不静默漂移
    port: 5273,
    strictPort: true,
    // 只绑 IPv4 回环：WebView2 解析 localhost 走 127.0.0.1
    host: '127.0.0.1',
    // 网页调试模式：命令与实时事件转发给 ai-manager-web（桌面 WebView 不走这层，
    // 它直接用 Tauri IPC；端口与 src-tauri/src/desktop/web.rs 的 WEB_PORT 成对）
    proxy: {
      '/cmd': { target: 'http://127.0.0.1:5274', changeOrigin: false },
      '/events': { target: 'http://127.0.0.1:5274', changeOrigin: false },
    },
    // 不监听运行期目录：target/ 里的 exe 被运行中的 app 占用会 EBUSY 把 Vite 监听器打崩，
    // data/ 的日志每次写入都会触发一轮无意义 HMR
    watch: {
      ignored: ['**/src-tauri/target/**', '**/data/**', '**/release/**'],
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'chrome120',
    // echarts 按需注册后单 chunk 仍 ~600KB（min）——已是三种图 + 六个组件的地板价，
    // 不再往下拆；放宽告警阈值免得每次构建都报
    chunkSizeWarningLimit: 700,
    rollupOptions: {
      output: {
        // echarts 单独成包：只有仪表盘页 import 它，主包与其他页面不加载这块；
        // 独立 chunk 也让升级 echarts 时其余页面缓存不失效
        manualChunks: { echarts: ['echarts'] },
      },
    },
  },
});
