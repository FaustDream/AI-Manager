// logs 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

/// 结构化日志查询（按级别 / 模块 / 关键词）。
/// 返回 `Result`：读日志失败要让界面说「读日志失败」，不能伪装成「没有符合条件的日志」。
#[tauri::command]

pub async fn log_query(
    filter: crate::app::log::LogFilter,
) -> std::result::Result<Vec<crate::app::log::Line>, String> {
    // query 是整读 app.jsonl 再逐行过滤，必须进阻塞池
    crate::app::runtime::blocking(move || crate::app::log::query(&filter))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 导出日志到文件；不给路径就落到 `data/logs/export-<时间戳>.txt`。
/// 返回一句可以直接显示的话（行数 + 落在哪）。
#[tauri::command]

pub async fn log_export(
    path: Option<String>,
    filter: crate::app::log::LogFilter,
) -> std::result::Result<String, String> {
    let target = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::app::paths::logs_dir().join(format!(
            "export-{}.txt",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        )),
    };
    let shown = target.display().to_string();
    let count = crate::app::runtime::blocking(move || crate::app::log::export(&target, &filter))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;
    Ok(format!("已导出 {count} 行到 {shown}"))
}
