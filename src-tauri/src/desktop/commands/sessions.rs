// sessions 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

// ------------------------------------------------------------ 会话

/// 增量扫描会话索引：扫完 emit `session:scan` 并返回报告。
pub async fn session_scan(
    state: &AppState,
) -> std::result::Result<crate::app::sessions::SessionScanReport, String> {
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
pub async fn session_page(
    state: &AppState,
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
pub async fn session_preview(
    state: &AppState,
    key: String,
) -> std::result::Result<crate::app::sessions::Preview, String> {
    let service = state.sessions.clone();
    service.preview(key).await.map_err(err_text)
}

/// 这批会话会临时关闭哪些**正在运行**的客户端（界面在确认框里点名）。
pub async fn session_running_clients(
    keys: Vec<String>,
) -> std::result::Result<Vec<String>, String> {
    crate::app::runtime::blocking(move || crate::app::sessions::delete::running_clients(&keys))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))
}

/// 当前筛选下的全部行 key（不分页），给「选中全部」用。
pub async fn session_keys(
    state: &AppState,
    filter: crate::app::sessions::Filter,
) -> std::result::Result<Vec<String>, String> {
    let service = state.sessions.clone();
    service.keys(filter).await.map_err(err_text)
}

/// 三个下拉的取值与计数（来源 / 账号 / 状态）。
pub async fn session_facets(
    state: &AppState,
) -> std::result::Result<crate::app::sessions::Facets, String> {
    let service = state.sessions.clone();
    service.facets().await.map_err(err_text)
}

/// 归档 / 取消归档。只写本地标记，不动客户端库，所以不用关客户端。
pub async fn session_archive(
    state: &AppState,
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
pub async fn session_purge(
    state: &AppState,
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
pub async fn session_retention(state: &AppState) -> std::result::Result<u32, String> {
    Ok(state.sessions.retention_days().await)
}

pub async fn session_retention_set(
    state: &AppState,
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

// ------------------------------------------------------------ v3：置顶 / 改名 / 隐藏 / 标签

/// 置顶 / 取消置顶。只写本地标记，客户端与扫描都无感。
pub async fn session_pin(
    state: &AppState,
    keys: Vec<String>,
    on: bool,
) -> std::result::Result<usize, String> {
    let count = state.sessions.pin(keys, on).await.map_err(err_text)?;
    log::log(
        Level::Info,
        "sessions",
        format!("{} {} 条会话", if on { "置顶" } else { "取消置顶" }, count),
    );
    Ok(count)
}

/// 本地改名（`title = None` 表示恢复客户端原标题）。改名即锁定：扫描永不覆盖。
pub async fn session_rename(
    state: &AppState,
    key: String,
    title: Option<String>,
) -> std::result::Result<(), String> {
    let action = match &title {
        Some(t) => format!("重命名会话：{t}"),
        None => "恢复会话原标题".to_string(),
    };
    state.sessions.rename(key, title).await.map_err(err_text)?;
    log::log(Level::Info, "sessions", action);
    Ok(())
}

/// 隐藏 / 取消隐藏。Trae 家族的「彻底删除」就是写隐藏墓碑，取消隐藏=恢复显示。
pub async fn session_hide(
    state: &AppState,
    keys: Vec<String>,
    on: bool,
) -> std::result::Result<usize, String> {
    let count = state.sessions.hide(keys, on).await.map_err(err_text)?;
    log::log(
        Level::Info,
        "sessions",
        format!("{} {} 条会话", if on { "隐藏" } else { "取消隐藏" }, count),
    );
    Ok(count)
}

/// 全量替换一个会话的标签。
pub async fn session_tag_set(
    state: &AppState,
    key: String,
    tags: Vec<String>,
) -> std::result::Result<usize, String> {
    let count = state.sessions.tags_set(key, tags).await.map_err(err_text)?;
    log::log(
        Level::Info,
        "sessions",
        format!("设置会话标签（{count} 个）"),
    );
    Ok(count)
}

// ------------------------------------------------------------ v3：导出 / 打开 / 建议 / 统计

/// 导出会话正文到 `data/exports/`（markdown / json），返回报告。
pub async fn session_export(
    state: &AppState,
    keys: Vec<String>,
    format: String,
) -> std::result::Result<crate::app::sessions::ExportReport, String> {
    let report = state
        .sessions
        .export(keys, &format)
        .await
        .map_err(err_text)?;
    log::log(
        Level::Info,
        "sessions",
        format!("导出会话：{}", report.path),
    );
    Ok(report)
}

/// 打开正文所在文件夹。正文在 SQLite 库里的来源（Qoder / ZCode / Trae）没有
/// 可打开的文件，返回 `false` 让界面提示，而不是硬开一个没用的目录。
pub async fn session_open_body(state: &AppState, key: String) -> std::result::Result<bool, String> {
    let dir = state.sessions.body_dir(key).await.map_err(err_text)?;
    let Some(dir) = dir else {
        return Ok(false);
    };
    crate::app::runtime::blocking(move || {
        crate::app::process::spawn_with_args(
            &std::path::PathBuf::from("explorer.exe"),
            &[dir.to_string_lossy().as_ref()],
            true,
        )
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?;
    Ok(true)
}

/// 打开该会话所属的客户端（复用账号中心的启动逻辑）。
/// 插件会话等没有独立客户端的来源返回 `false`。
pub async fn session_open_client(key: String) -> std::result::Result<bool, String> {
    let platform = crate::app::runtime::blocking(move || {
        crate::app::sessions::preview::parse_key(&key)
            .ok()
            .and_then(|t| {
                crate::app::sessions::delete::platform_of(&t.source_key).map(str::to_string)
            })
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?;
    let Some(platform) = platform else {
        return Ok(false);
    };
    crate::app::runtime::blocking(move || crate::app::accounts::add::open_client(&platform))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;
    Ok(true)
}

/// 未命名会话的标题建议（首条用户消息截短），返回 `[key, 建议]` 列表。
/// 纯只读；由界面在「一键治理」弹窗里人工确认后再调 `session_rename`。
pub async fn session_suggest_titles(
    state: &AppState,
    keys: Vec<String>,
) -> std::result::Result<Vec<(String, String)>, String> {
    state.sessions.suggest_titles(keys).await.map_err(err_text)
}

/// 会话维度统计（仪表盘「会话」区块）。
pub async fn session_stats(
    state: &AppState,
) -> std::result::Result<crate::app::sessions::SessionStats, String> {
    state.sessions.stats().await.map_err(err_text)
}
