// sessions 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

// ------------------------------------------------------------ 会话

/// 增量扫描会话索引：任务化，扫完 emit `session:scan`。
#[tauri::command]
pub async fn session_scan(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> std::result::Result<u64, String> {
    use crate::app::task::CapabilityId;
    use tauri::Emitter;

    let service = state.sessions.clone();
    let emitter = app.clone();
    let task_id = state.tasks.spawn_managed(
        CapabilityId::Sessions,
        "扫描会话索引",
        move |handle| async move {
            match service.scan(handle.cancel.clone()).await {
                Ok(report) => {
                    log::log(
                        Level::Info,
                        "sessions",
                        format!("会话扫描：{}", report.summary()),
                    );
                    handle.message(report.summary());
                    let _ = emitter.emit("session:scan", &report);
                    Ok(())
                }
                Err(e) => {
                    log::log(
                        Level::Warn,
                        "sessions",
                        format!("会话扫描失败：{}", e.message),
                    );
                    let _ = emitter.emit("session:scan", &e);
                    Err(e)
                }
            }
        },
    );
    Ok(task_id)
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

/// 删除会话：进回收站，可恢复。会临时关掉正在运行的客户端。
///
/// **走任务系统**：关客户端 + 搬文件动辄几十秒，同步命令会让按钮一直转圈且看不到进度。
/// 这里只返回任务 id，进度由 `task:progress` 推给界面，结果由 `session:delete` 事件带回。
#[tauri::command]

pub async fn session_delete(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    keys: Vec<String>,
) -> std::result::Result<u64, String> {
    use crate::app::task::CapabilityId;
    use tauri::Emitter;

    let count = keys.len();
    let service = state.sessions.clone();
    let emitter = app.clone();
    let task_id = state.tasks.spawn_managed(
        CapabilityId::Sessions,
        format!("删除 {count} 条会话"),
        move |handle| async move {
            let report = service.remove(keys).await?;
            log::log(
                if report.errors.is_empty() {
                    Level::Info
                } else {
                    Level::Warn
                },
                "sessions",
                format!("删除会话：{}", report.summary()),
            );
            handle.message(report.summary());
            let _ = emitter.emit("session:delete", &report);
            Ok(())
        },
    );
    Ok(task_id)
}

/// 整库快照（备份与清理页也会用）。
#[tauri::command]

pub async fn session_backup(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::sessions::backup::Snapshot, String> {
    let service = state.sessions.clone();
    let snapshot = service.backup().await.map_err(err_text)?;
    log::log(
        Level::Info,
        "sessions",
        format!("会话快照：{}", snapshot.summary()),
    );
    Ok(snapshot)
}

#[tauri::command]

pub async fn session_backups(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::sessions::backup::Snapshot>, String> {
    let service = state.sessions.clone();
    Ok(service.backups().await)
}

/// 回收站批次列表。
#[tauri::command]

pub async fn session_trash(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::sessions::trash::Manifest>, String> {
    let service = state.sessions.clone();
    Ok(service.trash().await)
}

/// 按批次恢复。
#[tauri::command]

pub async fn session_trash_restore(
    state: State<'_, AppState>,
    tag: String,
) -> std::result::Result<crate::app::sessions::delete::RestoreReport, String> {
    let service = state.sessions.clone();
    service.trash_restore(tag).await.map_err(err_text)
}

/// 彻底清掉一个批次，不可恢复。
#[tauri::command]

pub async fn session_trash_purge(
    state: State<'_, AppState>,
    tag: String,
) -> std::result::Result<i64, String> {
    let service = state.sessions.clone();
    service.trash_purge(tag).await.map_err(err_text)
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

/// 彻底删除：只放行已归档 / 已删除的会话，不留回收站副本。
/// 同样走任务系统：每条删完推一次进度，界面有进度条可看。
#[tauri::command]

pub async fn session_purge(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    keys: Vec<String>,
) -> std::result::Result<u64, String> {
    use crate::app::task::CapabilityId;
    use tauri::Emitter;

    let count = keys.len();
    let service = state.sessions.clone();
    let emitter = app.clone();
    let task_id = state.tasks.spawn_managed(
        CapabilityId::Sessions,
        format!("彻底删除 {count} 条会话"),
        move |handle| async move {
            let progress_handle = handle.clone();
            let report = service
                .purge_with(keys, move |done, total| {
                    progress_handle
                        .progress(done as f32 / total.max(1) as f32, format!("{done}/{total}"));
                })
                .await?;
            log::log(
                Level::Info,
                "sessions",
                format!(
                    "彻底删除 {} 条会话（{} 个文件）{}{}",
                    report.deleted,
                    report.files,
                    if report.refused > 0 {
                        format!("，{} 条未归档已跳过", report.refused)
                    } else {
                        String::new()
                    },
                    // 失败原因必须落日志：以前只报「0 条」，用户和开发者都看不出为什么
                    if report.errors.is_empty() {
                        String::new()
                    } else {
                        format!("，{} 条失败（{}）", report.errors.len(), report.errors[0])
                    }
                ),
            );
            handle.message(if report.errors.is_empty() {
                format!(
                    "已彻底删除 {} 条（{} 个文件）",
                    report.deleted, report.files
                )
            } else {
                format!(
                    "已彻底删除 {} 条，{} 条失败：{}",
                    report.deleted,
                    report.errors.len(),
                    report.errors[0]
                )
            });
            let _ = emitter.emit("session:purge", &report);
            Ok(())
        },
    );
    Ok(task_id)
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
