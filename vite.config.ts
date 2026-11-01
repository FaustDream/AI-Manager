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
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'chrome120',
  },
});
