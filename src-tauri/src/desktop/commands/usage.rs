// usage 的 IPC 命令：只做转接，采集与聚合在 app/usage。

use super::*;
use crate::app::error::ErrorKind;

/// 手动扫一轮本地用量（只读客户端文件，不代理、不改任何配置）。
#[tauri::command]
pub async fn usage_scan(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::usage::UsageScanReport, String> {
    let db = state.db.clone();
    let report = crate::app::runtime::blocking(move || {
        crate::app::usage::scan_all(&db, &crate::app::clients::Roots::system())
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    log::action(
        "usage_scan",
        "user",
        report.errors.is_empty(),
        report.ms as u64,
        format!(
            "用量采集：扫描 {} 条 · 新增 {} 条{}",
            report.scanned,
            report.inserted,
            if report.errors.is_empty() {
                String::new()
            } else {
                format!(" · 失败 {}", report.errors.len())
            }
        ),
    );
    Ok(report)
}

/// 用量总览：按天 / 按平台 / 按模型。
#[tauri::command]
pub async fn usage_overview(
    state: State<'_, AppState>,
    days: Option<i64>,
) -> std::result::Result<crate::app::usage::UsageOverview, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::usage::overview(&db, days.unwrap_or(14)))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}
