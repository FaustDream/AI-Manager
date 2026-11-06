// settings 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

#[tauri::command]

pub async fn settings_get(
    state: State<'_, AppState>,
    key: String,
) -> std::result::Result<Option<serde_json::Value>, String> {
    let db = state.db.clone();
    let raw = crate::app::runtime::blocking(move || db.kv_get(&key))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;
    Ok(raw.and_then(|text| serde_json::from_str(&text).ok()))
}

#[tauri::command]

pub async fn settings_set(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> std::result::Result<(), String> {
    let db = state.db.clone();
    let text = value.to_string();
    crate::app::runtime::blocking(move || {
        // 写前留一份旧值：设置页改错了能回滚（`ui.*` 不参与）
        crate::app::config_history::capture(&db, &key, Some(&text))?;
        db.kv_set(&key, &text)
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)
}

#[tauri::command]

pub fn autostart_get() -> bool {
    crate::app::process::autostart_enabled()
}

#[tauri::command]

pub fn autostart_set(enabled: bool) -> std::result::Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?;
    crate::app::process::set_autostart(enabled, &exe).map_err(err_text)
}

#[tauri::command]

pub fn log_level(level: String) -> std::result::Result<(), String> {
    let parsed = Level::parse_lossy(&level);
    log::set_level(parsed);
    Ok(())
}
