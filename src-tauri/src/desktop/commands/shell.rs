// shell 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

/// 前端挂载完成后回调一次。
/// 意义不在于「打个招呼」，而在于让 Rust 侧能确认 IPC 通道真的通了——
/// 窗口能开但 `__TAURI__` 没注进来的情况，只看窗口是看不出来的。
#[tauri::command]

pub fn ui_ready(state: State<'_, AppState>) -> String {
    log::log(Level::Info, "ui", "前端已就绪，IPC 通道正常");
    format!("{}|{}", env!("CARGO_PKG_VERSION"), state.db_path_label())
}

#[tauri::command]

pub fn app_quit(app: tauri::AppHandle) {
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

/// 首屏渲染完成：把窗口显示出来。
/// 窗口在配置里是 `visible: false`——WebView2 初始化那几百毫秒是一块白底，
/// 直接显示就是「白屏启动」。前端在首屏画好之后调这里，此时窗口里已经有启动画面。
#[tauri::command]

pub fn app_show_window(window: tauri::Window) {
    let _ = window.show();
}
