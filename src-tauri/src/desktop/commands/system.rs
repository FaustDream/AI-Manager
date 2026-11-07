// system 的 IPC 命令：健康报告、诊断包、更新检测、配置快照。
// 全部只做转接，业务在 app/health、app/update、app/config_history。

use super::*;
use crate::app::error::ErrorKind;

/// 健康报告。顺带刷新托盘提示——窗口最小化时，托盘是唯一还能看见应用的地方。
#[tauri::command]
pub async fn system_health(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::health::HealthReport, String> {
    let db = state.db.clone();
    let report = crate::app::runtime::blocking(move || crate::app::health::report(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;

    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(format!("AI Manager v{}", report.version)));
    }
    Ok(report)
}

/// 导出诊断包，返回文件路径。前端只负责提示「在哪」。
#[tauri::command]
pub async fn diagnose_bundle(state: State<'_, AppState>) -> std::result::Result<String, String> {
    let db = state.db.clone();
    let path = crate::app::runtime::blocking(move || crate::app::health::export_bundle(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;
    log::action(
        "diagnose_bundle",
        "user",
        true,
        0,
        format!("诊断包已导出：{}", path.display()),
    );
    Ok(path.display().to_string())
}

/// 检查更新：只查 GitHub Releases，不下载不替换。
#[tauri::command]
pub async fn update_check() -> std::result::Result<crate::app::update::UpdateInfo, String> {
    let http = crate::app::net::RealHttp::new();
    crate::app::update::check(&http, env!("CARGO_PKG_VERSION"))
        .await
        .map_err(err_text)
}

#[tauri::command]
pub async fn config_snapshot_list(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::config_history::Snapshot>, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::config_history::list(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 回滚一条配置快照。`schedules` 回滚后要唤醒调度循环，否则新规则要等重启才生效。
#[tauri::command]
pub async fn config_snapshot_restore(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<crate::app::config_history::Snapshot, String> {
    let db = state.db.clone();
    let restored =
        crate::app::runtime::blocking(move || crate::app::config_history::restore(&db, &id))
            .await
            .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
            .map_err(err_text)?;
    if restored.key == "schedules" {
        crate::app::schedule::notify_changed();
    }
    log::action(
        "config_restore",
        "user",
        true,
        0,
        format!("配置已回滚：{}", restored.key),
    );
    Ok(restored)
}

#[tauri::command]
pub async fn config_snapshot_clear(state: State<'_, AppState>) -> std::result::Result<(), String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::config_history::clear(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}
