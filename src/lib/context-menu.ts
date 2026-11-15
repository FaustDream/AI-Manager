// 关闭浏览器 / WebView 的原生右键菜单。
//
// 桌面应用里原生菜单（后退/刷新/另存为/检查元素）几乎全是干扰项，这里在
// document 的捕获阶段拦 `contextmenu`：**只 preventDefault**，不 stopPropagation——
// 页面自己的自定义右键菜单（会话行的操作菜单）靠冒泡阶段拿到同一个事件来打开，
// 一旦在这里断流，应用菜单永远弹不出来（实测踩坑：会话页右键无响应就是它）。
// preventDefault 已经足以压掉原生菜单，后续处理器再多也改变不了这一点。
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
  // 只 preventDefault：原生菜单不弹；事件继续冒泡，让应用的自定义菜单接得住。
  event.preventDefault();
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
