// sessions 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

// ------------------------------------------------------------ 会话

/// 增量扫描会话索引：扫完 emit `session:scan` 并返回报告。
#[tauri::command]
pub async fn session_scan(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::sessions::ScanReport, String> {
    let service = state.sessions.clone();
    let report = match service.scan().await {
        Ok(report) => {
            log::log(
                Level::Info,
                "sessions",
                format!("会话扫描：{}", report.summary()),
            );
            crate::app::events::emit("session:scan", &report);
            report
        }
        Err(e) => {
            log::log(
                Level::Warn,
                "sessions",
                format!("会话扫描失败：{}", e.message),
            );
            return Err(err_text(e));
        }
    };
    Ok(report)
}

/// 列表页：只读 app.db，客户端正在写也不影响。
#[tauri::command]

pub async fn session_page(
    state: State<'_, AppState>,
    cursor: Option<String>,
    limit: Option<usize>,
    filter: Option<crate::app::sessions::Filter>,
) -> std::result::Result<crate::types::Page<crate::types::SessionRow>, String> {
    let service = state.sessions.clone();
    service
        .page(filter.unwrap_or_default(), cursor, limit.unwrap_or(100))
        .await
        .map_err(err_text)
}

/// 正文预览：按行 key 找正文。只读、只取头尾。
#[tauri::command]

pub async fn session_preview(
    state: State<'_, AppState>,
    key: String,
) -> std::result::Result<crate::app::sessions::Preview, String> {
    let service = state.sessions.clone();
    service.preview(key).await.map_err(err_text)
}

/// 这批会话会临时关闭哪些**正在运行**的客户端（界面在确认框里点名）。
#[tauri::command]

pub async fn session_running_clients(
    keys: Vec<String>,
) -> std::result::Result<Vec<String>, String> {
    crate::app::runtime::blocking(move || crate::app::sessions::delete::running_clients(&keys))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))
}

/// 当前筛选下的全部行 key（不分页），给「选中全部」用。
#[tauri::command]

pub async fn session_keys(
    state: State<'_, AppState>,
    filter: crate::app::sessions::Filter,
) -> std::result::Result<Vec<String>, String> {
    let service = state.sessions.clone();
    service.keys(filter).await.map_err(err_text)
}

/// 三个下拉的取值与计数（来源 / 账号 / 状态）。
#[tauri::command]

pub async fn session_facets(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::sessions::Facets, String> {
    let service = state.sessions.clone();
    service.facets().await.map_err(err_text)
}

/// 归档 / 取消归档。只写本地标记，不动客户端库，所以不用关客户端。
#[tauri::command]

pub async fn session_archive(
    state: State<'_, AppState>,
    keys: Vec<String>,
    on: bool,
) -> std::result::Result<usize, String> {
    let service = state.sessions.clone();
    let count = service.archive(keys, on).await.map_err(err_text)?;
    log::log(
        Level::Info,
        "sessions",
        format!("{} {} 条会话", if on { "归档" } else { "取消归档" }, count),
    );
    Ok(count)
}

/// 彻底删除：只放行已归档或特殊状态（已删除 / 云端 / 旧账号）的会话，不留副本。
///
/// 这是不可逆动作，门槛放在后端而不是只靠界面置灰：绕过界面直接调命令也删不掉
/// 一条还在用的会话。删完返回报告并 emit `session:purge`。
#[tauri::command]

pub async fn session_purge(
    state: State<'_, AppState>,
    keys: Vec<String>,
) -> std::result::Result<crate::app::sessions::delete::PurgeReport, String> {
    let service = state.sessions.clone();
    let report = service.purge(keys).await.map_err(err_text)?;
    // 一条都没删掉就是彻底失败：以前这里固定 Info，「135 条全部失败」在日志页
    // 按「只看错误」筛不出来，用户只能看到一句「删除完成」。
    let errors = report.errors.len();
    let level = if errors == 0 && report.refused == 0 {
        Level::Info
    } else if report.deleted == 0 && errors > 0 {
        Level::Error
    } else {
        Level::Warn
    };
    let detail = if errors == 0 {
        String::new()
    } else {
        // 前三条足够定位共性问题（多半是同一个跨盘 / 占用原因），全量在 jsonl 里
        format!(
            " · 失败明细={}",
            report
                .errors
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("；")
        )
    };
    let summary = format!(
        "彻底删除 {} 条会话（{} 个文件）{}{}{}",
        report.deleted,
        report.files,
        if report.refused > 0 {
            format!("，{} 条未归档已跳过", report.refused)
        } else {
            String::new()
        },
        if errors == 0 {
            String::new()
        } else {
            format!("，{errors} 条失败")
        },
        detail
    );
    if level == Level::Error {
        log::fail(
            "sessions",
            "session_purge",
            &summary,
            format!(
                "删除={} 文件={} 失败={errors}",
                report.deleted, report.files
            ),
        );
    } else {
        log::log(level, "sessions", summary);
    }
    crate::app::events::emit("session:purge", &report);
    Ok(report)
}

/// 自动归档的保留天数（0 = 关闭）。
#[tauri::command]

pub async fn session_retention(state: State<'_, AppState>) -> std::result::Result<u32, String> {
    Ok(state.sessions.retention_days().await)
}

#[tauri::command]

pub async fn session_retention_set(
    state: State<'_, AppState>,
    days: u32,
) -> std::result::Result<u32, String> {
    let days = state
        .sessions
        .set_retention_days(days)
        .await
        .map_err(err_text)?;
    log::log(
        Level::Info,
        "sessions",
        if days == 0 {
            "自动归档已关闭".to_string()
        } else {
            format!("自动归档：超过 {days} 天未活动的会话")
        },
    );
    Ok(days)
}
