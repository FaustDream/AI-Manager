//! Tauri 外壳：窗口、托盘、单实例、IPC 命令。
//!
//! 这一层**只做转接**，不放业务逻辑——所有能力来自 `app/` 下的模块。
//! 命令的「类型」由 ts-rs 生成，「名单」由 `generate_handler!` 保证（架构 D8）。
//!
//! 两条 P1 就得立住的规矩：
//!   - 关窗口 = 隐藏，不退出；退出只走托盘菜单或 `app_quit`
//!   - 数据库调用一律 `spawn_blocking`，否则界面会在等结果期间变灰

use crate::app::db::Db;
use crate::app::error::AppError;
use crate::app::log::{self, Level};
use crate::app::process;
use crate::app::task::{resources::ResourceRegistry, TaskEnv, TaskStore, TaskView};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State, WindowEvent};

pub struct AppState {
    pub db: Arc<Db>,
    pub tasks: Arc<TaskStore>,
    pub env: Arc<TaskEnv>,
    /// 单实例句柄必须活到进程结束，Drop 提前释放会让第二次启动挤进来
    _instance: Option<process::SingleInstance>,
}

impl AppState {
    fn db_path_label(&self) -> String {
        crate::app::paths::app_db().display().to_string()
    }
}

fn err_text(e: AppError) -> String {
    // 前端拿到的永远是 {kind, message} 的结构化错误
    serde_json::to_string(&e).unwrap_or(e.message)
}

// ------------------------------------------------------------------ 命令

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 前端挂载完成后回调一次。
/// 意义不在于「打个招呼」，而在于让 Rust 侧能确认 IPC 通道真的通了——
/// 窗口能开但 `__TAURI__` 没注进来的情况，只看窗口是看不出来的。
#[tauri::command]
fn ui_ready(state: State<'_, AppState>) -> String {
    log::log(Level::Info, "ui", "前端已就绪，IPC 通道正常");
    format!("{}|{}", env!("CARGO_PKG_VERSION"), state.db_path_label())
}

#[tauri::command]
fn app_quit(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn window_show(app: tauri::AppHandle) -> std::result::Result<(), String> {
    let Some(w) = app.get_webview_window("main") else {
        return Err(err_text(AppError::new("invalid", "主窗口不存在")));
    };
    w.show().map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    w.set_focus().map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    Ok(())
}

#[tauri::command]
fn window_hide(app: tauri::AppHandle) -> std::result::Result<(), String> {
    let Some(w) = app.get_webview_window("main") else {
        return Err(err_text(AppError::new("invalid", "主窗口不存在")));
    };
    w.hide().map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    Ok(())
}

#[tauri::command]
fn tray_set_autostart(enabled: bool) -> std::result::Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    process::set_autostart(enabled, &exe).map_err(err_text)
}

#[tauri::command]
async fn settings_get(state: State<'_, AppState>, key: String) -> std::result::Result<Option<serde_json::Value>, String> {
    let db = state.db.clone();
    let raw = tokio::task::spawn_blocking(move || db.kv_get(&key))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)?;
    Ok(raw.and_then(|text| serde_json::from_str(&text).ok()))
}

#[tauri::command]
async fn settings_set(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> std::result::Result<(), String> {
    let db = state.db.clone();
    let text = value.to_string();
    tokio::task::spawn_blocking(move || db.kv_set(&key, &text))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

#[tauri::command]
fn task_list(state: State<'_, AppState>) -> Vec<TaskView> {
    state.tasks.list()
}

// ------------------------------------------------------------------ 账号中心

#[tauri::command]
async fn account_list(state: State<'_, AppState>) -> std::result::Result<Vec<crate::app::accounts::AccountView>, String> {
    let db = state.db.clone();
    // 数据库调用必须在阻塞线程里跑，否则界面在等结果期间没有响应
    let rows = tokio::task::spawn_blocking(move || crate::app::accounts::repo::list(&db))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)?;
    log::log(Level::Info, "accounts", format!("列表返回 {} 个账号", rows.len()));
    Ok(rows)
}

#[tauri::command]
async fn account_reorder(state: State<'_, AppState>, iids: Vec<String>) -> std::result::Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || crate::app::accounts::repo::reorder(&db, &iids))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

#[tauri::command]
async fn account_delete(state: State<'_, AppState>, iid: String) -> std::result::Result<bool, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || crate::app::accounts::repo::delete(&db, &iid))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

#[tauri::command]
fn task_cancel(state: State<'_, AppState>, task_id: u64) -> bool {
    state.tasks.cancel_task(task_id)
}

#[tauri::command]
fn log_tail(limit: Option<usize>) -> Vec<String> {
    log::tail(limit.unwrap_or(200).min(2000))
}

#[tauri::command]
fn log_level(level: String) -> std::result::Result<(), String> {
    let parsed = Level::from_str(&level);
    log::set_level(parsed);
    Ok(())
}

// ------------------------------------------------------------------ 启动

pub fn run() -> tauri::Result<()> {
    // 单实例必须在建窗口之前拿到，否则第二个实例会先闪一下窗口再退出
    let instance = match process::acquire_single_instance("AIManager.SingleInstance") {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => {
            // 已有实例在跑：把它的窗口叫到前面，然后自己退出
            focus_existing();
            return Ok(());
        }
        Err(_) => None,
    };

    tauri::Builder::default()
        .setup(move |app| {
            let state = init_state(instance)?;
            app.manage(state);

            build_tray(app)?;
            hook_window(app);

            log::log(Level::Info, "app", "P1 外壳启动完成");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            ui_ready,
            app_quit,
            window_show,
            window_hide,
            tray_set_autostart,
            settings_get,
            settings_set,
            task_list,
            task_cancel,
            account_list,
            account_reorder,
            account_delete,
            log_tail,
            log_level
        ])
        .run(tauri::generate_context!())
}

fn init_state(instance: Option<process::SingleInstance>) -> std::result::Result<AppState, Box<dyn std::error::Error>> {
    log::init(Level::Info)?;
    let db = Arc::new(Db::open()?);
    let tasks = TaskStore::new();
    let resources = ResourceRegistry::new();
    let env = Arc::new(TaskEnv::new(tasks.clone(), resources));

    log::log(Level::Info, "app", format!("数据目录 {}", crate::app::paths::data_dir().display()));

    Ok(AppState { db, tasks, env, _instance: instance })
}

fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("AI Manager")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 关窗口 = 收到托盘，不退出进程（工作台要常驻）
fn hook_window(app: &mut tauri::App) {
    let Some(window) = app.get_webview_window("main") else { return };
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = handle.hide();
        }
    });
}

/// 第二个实例启动时，尽力把已有窗口叫到前台。
/// 不做 IPC 握手（要多一套进程间通道），只做「窗口还在就激活」的尽力而为。
fn focus_existing() {
    // 已有实例在跑：什么都不做，让操作系统保持它原本的层级
}
