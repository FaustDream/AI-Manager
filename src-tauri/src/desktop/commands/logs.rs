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

/// 记录一次用户动作（执行记录）。前端命令面在每次调用后回传结果。
///
/// 走独立命令而不是让每个业务命令自己写日志：命令有几十条，只有前端知道
/// 「用户点了什么、参数是什么、等了多久」，后端各写各的必然漏。
#[tauri::command]

pub fn log_action(
    action: String,
    trigger: String,
    ok: bool,
    ms: u64,
    detail: String,
) -> std::result::Result<(), String> {
    crate::app::log::action(&action, &trigger, ok, ms, detail);
    Ok(())
}

/// 界面侧的日志：读失败、前端异常、命令报错都走这里。
///
/// **不进执行记录**（读不是「动作」），但必须留痕且必须带证据：界面上只剩一句
/// toast 时，`evidence` 里的错误原文是唯一能定位问题的东西。
#[tauri::command]

pub fn log_ui(
    level: String,
    action: Option<String>,
    message: String,
    evidence: Option<String>,
) -> std::result::Result<(), String> {
    crate::app::log::ui(
        crate::app::log::Level::parse_lossy(&level),
        action.as_deref(),
        message,
        evidence.as_deref(),
    );
    Ok(())
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
