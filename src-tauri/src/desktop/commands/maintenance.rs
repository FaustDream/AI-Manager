// maintenance 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

/// 数据目录占用。
#[tauri::command]

pub async fn maintenance_space() -> crate::app::maintenance::SpaceReport {
    // 递归遍历整个 data/（含会话正文），主线程上跑会卡住界面
    crate::app::runtime::blocking(crate::app::maintenance::space)
        .await
        .unwrap_or_default()
}

/// 回收站批次（会话删除与平台技能移除都算）。
#[tauri::command]

pub async fn maintenance_trash() -> Vec<crate::app::maintenance::SpaceEntry> {
    crate::app::runtime::blocking(crate::app::maintenance::trash_batches)
        .await
        .unwrap_or_default()
}

/// 清空回收站：`tag=None` 全清。返回释放的字节数。
#[tauri::command]

pub async fn maintenance_purge_trash(tag: Option<String>) -> std::result::Result<i64, String> {
    crate::app::runtime::blocking(move || crate::app::maintenance::purge_trash(tag.as_deref()))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 打开数据目录。
#[tauri::command]

pub fn maintenance_open_data() -> std::result::Result<(), String> {
    crate::app::maintenance::open_data_dir().map_err(err_text)
}

/// 迁移收尾第一步：三项计数校验。只读，随时可点。
#[tauri::command]

pub async fn migrate_verify(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::migrate::verify::MigrationReport, String> {
    let db = state.db.clone();
    // 读 accounts.dat 要过 DPAPI、还要解 JSON，纯阻塞活，不能占着异步线程
    let report = crate::app::runtime::blocking(move || {
        crate::app::migrate::verify::verify(&db, &crate::app::paths::data_dir())
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    log::log(
        if report.ok { Level::Info } else { Level::Warn },
        "migrate",
        format!("迁移校验：{}", report.describe()),
    );
    Ok(report)
}

/// 迁移收尾第二步：**复制** v1 文件到 `data-v1-archive/<时间戳>/`。
/// 计数不一致直接拒绝，什么都不写；原文件永不移动、永不删除。
#[tauri::command]

pub async fn migrate_archive(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::migrate::verify::ArchiveReport, String> {
    let db = state.db.clone();
    let report = crate::app::runtime::blocking(move || {
        crate::app::migrate::verify::archive(&db, &crate::app::paths::data_dir())
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    log::log(
        Level::Info,
        "migrate",
        format!("v1 文件已归档到 {}", report.archive_dir),
    );
    Ok(report)
}
