// 空闲调度：把「不急但要花时间」的活挪到首屏之后。
//
// 有 requestIdleCallback 就用它（浏览器空闲帧才跑，不抢首屏的带宽与主线程）；
// 没有就退化成 setTimeout。两种情况都给超时兜底，避免空闲帧一直不来导致永不执行。
export function onIdle(run: () => void, timeout = 1500): void {
  if (typeof window === 'undefined') return;
  if (typeof window.requestIdleCallback === 'function') {
    window.requestIdleCallback(run, { timeout });
    return;
  }
  window.setTimeout(run, 200);
}
