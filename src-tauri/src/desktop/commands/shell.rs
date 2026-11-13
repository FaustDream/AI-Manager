// shell 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

/// 前端挂载完成后回调一次。
/// 意义不在于「打个招呼」，而在于让 Rust 侧能确认 IPC 通道真的通了——
/// 窗口能开但 `__TAURI__` 没注进来的情况，只看窗口是看不出来的。
///
/// 只有**本进程第一次**挂载记 Info：开发期热更新、托盘「重载界面」都会重新挂载，
/// 每次都写一条会把日志刷成同一句话。后续挂载是过程性事件，降到 Debug。
#[tauri::command]

pub fn ui_ready(state: State<'_, AppState>) -> String {
    static MOUNTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let nth = MOUNTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if nth == 1 {
        log::log(Level::Info, "ui", "前端已就绪，IPC 通道正常");
    } else {
        log::log(Level::Debug, "ui", "界面重新挂载，IPC 通道正常");
    }
    format!("{}|{}", env!("CARGO_PKG_VERSION"), state.db_path_label())
}

/// 当前程序版本（设置页展示用，避免前端 app 插件权限问题）。
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]

pub fn app_quit(app: tauri::AppHandle) {
    // 退出前把「重复 N 次」的计数补写掉，否则最后一串只剩首行
    log::flush();
    app.exit(0);
}

/// 用系统默认浏览器打开一个链接（登录页的「在浏览器打开」）。
/// 只放行 http/https：这个命令的入参来自界面，不能变成任意协议的启动器。
#[tauri::command]

pub fn open_external(url: String) -> std::result::Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err(err_text(AppError::new(
            ErrorKind::Invalid,
            "只能打开 http/https 链接",
        )));
    }
    if !crate::app::process::open_url(trimmed) {
        return Err(err_text(AppError::new(ErrorKind::Io, "没能打开浏览器")));
    }
    Ok(())
}

/// 用**无痕窗口**打开登录页（重新登录 / 添加账号的登录链接）。
///
/// 为什么不用 `open_external`：复用浏览器已有会话时，平台会按「已登录的那个账号」
/// 直接签发凭据，用户点「重新登录 A」却拿回 B 的凭据，账号库就此串掉。
///
/// 返回实际使用的浏览器名，界面据此说明「去哪个窗口里登录」；
/// 找不到支持无痕参数的浏览器时返回 Unsupported，界面退回普通打开。
#[tauri::command]

pub fn open_private_browser(url: String) -> std::result::Result<String, String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err(err_text(AppError::new(
            ErrorKind::Invalid,
            "只能打开 http/https 链接",
        )));
    }
    crate::app::browser::open_private(trimmed)
        .map(|browser| browser.to_string())
        .map_err(err_text)
}

/// 首屏渲染完成：把窗口显示出来。
/// 窗口在配置里是 `visible: false`——WebView2 初始化那几百毫秒是一块白底，
/// 直接显示就是「白屏启动」。前端在首屏画好之后调这里，此时窗口里已经有启动画面。
#[tauri::command]

pub fn app_show_window(window: tauri::Window) {
    // 自启静默启动时，前端首屏渲染完也不弹窗——保持托盘
    if crate::desktop::START_MINIMIZED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let _ = window.show();
}
