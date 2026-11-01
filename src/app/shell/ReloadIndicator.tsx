// 重载指示层：托盘「重载界面」触发的可视反馈。
// 重载是瞬间动作，没有这个层用户不知道有没有生效（实测反馈）。
// 用 sessionStorage 跨重载传状态：reload 前写标记，新页面加载完读到就播放
// 一次入场动画再自动隐藏；标记只活一个页面周期，正常启动不显示。
const KEY = 'aim.reloading';

export function markReloading(): void {
  try {
    sessionStorage.setItem(KEY, '1');
  } catch {
    /* 隐私模式等场景写不进就算了：指示层只是锦上添花 */
  }
}

export function ReloadIndicator() {
  let wasReloading = false;
  try {
    wasReloading = sessionStorage.getItem(KEY) === '1';
    sessionStorage.removeItem(KEY);
  } catch {
    /* 同上 */
  }
  if (!wasReloading) return null;
  return (
    <div className="pointer-events-none fixed inset-0 z-[70] flex items-center justify-center bg-bg/70 backdrop-blur-sm view-enter">
      <div className="flex items-center gap-2.5 rounded-lg border border-line bg-card px-5 py-3 shadow-lg">
        <svg className="h-4 w-4 animate-spin text-accent" viewBox="0 0 24 24" fill="none" aria-hidden>
          <circle cx="12" cy="12" r="10" stroke="currentColor" strokeOpacity="0.25" strokeWidth="4" />
          <path d="M22 12a10 10 0 0 0-10-10" stroke="currentColor" strokeWidth="4" strokeLinecap="round" />
        </svg>
        <span className="text-sm font-medium">正在重载界面…</span>
      </div>
    </div>
  );
}
