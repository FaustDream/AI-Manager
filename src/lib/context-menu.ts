// 关闭浏览器 / WebView 的原生右键菜单。
//
// 桌面应用里原生菜单（后退/刷新/另存为/检查元素）几乎全是干扰项，这里在
// document 的捕获阶段拦 `contextmenu`：preventDefault 让菜单不弹，
// stopPropagation 防止页面内其他 contextmenu 处理器再把事件捡起来。
//
// 只拦右键菜单这一种事件：键盘、焦点、文本选择、拖拽等基础交互完全不受影响，
// 复制/粘贴仍可用 Ctrl+C / Ctrl+V。
//
// 兼容性：`contextmenu` + `addEventListener(..., { capture: true })` 在
// Chrome / Edge / Firefox / Safari（含 iOS 长按）上都成立，不依赖
// `window.oncontextmenu` 赋值那一套旧写法。

/** 总开关：改成 false 即可恢复原生右键菜单，调用点无需改动。 */
export const DISABLE_NATIVE_CONTEXT_MENU = true;

/** 只依赖 add/removeEventListener，便于在 node 测试里替换成假目标。 */
export interface ContextMenuTarget {
  addEventListener(
    type: 'contextmenu',
    listener: (event: Event) => void,
    options?: AddEventListenerOptions | boolean,
  ): void;
  removeEventListener(
    type: 'contextmenu',
    listener: (event: Event) => void,
    options?: AddEventListenerOptions | boolean,
  ): void;
}

function blockContextMenu(event: Event): void {
  // 两件事都必须做：preventDefault 决定「弹不弹」，stopPropagation 决定
  // 「还有没有人能接到」。只做一半都会留下漏网的菜单。
  event.preventDefault();
  event.stopPropagation();
}

/**
 * 安装右键菜单守卫，返回卸载函数（HMR / 测试用）。
 *
 * @param enabled 缺省读总开关；显式传 false 可临时恢复原生菜单
 * @param target  缺省 document；抽成参数是为了能用假目标做行为断言
 */
export function installContextMenuGuard(
  enabled: boolean = DISABLE_NATIVE_CONTEXT_MENU,
  target: ContextMenuTarget = document,
): () => void {
  if (!enabled) return () => {};
  target.addEventListener('contextmenu', blockContextMenu, { capture: true });
  return () => target.removeEventListener('contextmenu', blockContextMenu, { capture: true });
}
