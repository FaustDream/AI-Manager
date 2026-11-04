// 启动画面的收尾。放独立模块是因为它要被两处调用：
//   main.tsx —— 兜底超时（页面 chunk 加载失败时也不能永远停在启动画面上）
//   Shell 里的 Suspense 内部 —— 首屏页面真正渲染出来之后再撤
// 两处都调同一个幂等函数，谁先到谁生效。

let done = false;

export function hideSplash(): void {
  if (done) return;
  done = true;
  const splash = document.getElementById('splash');
  if (!splash) return;
  splash.setAttribute('data-hide', '1');
  // 与 index.html 里的 transition 时长对齐，淡完再摘掉节点
  window.setTimeout(() => splash.remove(), 240);
}

/** 兜底：最多等这么久就强制撤掉，避免异常时停在启动画面上。 */
export function armSplashTimeout(maxMs = 6000): void {
  window.setTimeout(hideSplash, maxMs);
}
