//! 会话永久删除：关客户端 → 删正文与库行 → 清索引 → 重启客户端。
//!
//! 回收站与恢复已下线：这里只做不可逆清理。门槛在 `Service::purge`——
//! 只有已归档或特殊状态（客户端已删除 / 云端占位 / 旧账号）的会话能走到这里，
//! 可见会话必须先归档。

use crate::app::accounts::add::client_exe;
pub use crate::app::clients::checkpoint;
use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::process::start_detached;
use crate::app::sessions::preview;
use crate::app::sessions::sources::wb;
use crate::app::sessions::store::SqliteSessions;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct PurgeReport {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub deleted: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub files: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes: i64,
    /// 被门槛挡下的可见会话数（可见会话必须先归档）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub refused: usize,
    pub errors: Vec<String>,
    /// 关/重启客户端的告警：重启失败必须让用户看见，否则就是「客户端被关了没起来」
    pub warnings: Vec<String>,
}

/// 来源 → 需要先关掉的客户端平台键（`accounts::add` 用的是同一套键）。
fn platform_of(source: &str) -> Option<&'static str> {
    match source {
        "cn" => Some("workbuddy"),
        "intl" => Some("workbuddy_intl"),
        "qoder_cn" => Some("qoder"),
        "qoder_intl" => Some("qoder_intl"),
        "zcode" => Some("zcode"),
        // CodeBuddy 只动文件，不需要关 IDE
        _ => None,
    }
}

/// 关掉这些平台里正在跑的客户端，返回（被关掉的平台键, 关闭告警）。
pub fn close_clients(platforms: &[&str]) -> (Vec<String>, Vec<String>) {
    let mut closed = Vec::new();
    let mut warnings = Vec::new();
    for platform in platforms {
        match crate::app::accounts::add::close_client(platform) {
            Ok(true) => {
                crate::app::log::log(
                    crate::app::log::Level::Info,
                    "sessions",
                    format!(
                        "写客户端库前先关闭 {} 客户端（删完会自动重启）",
                        super::backend::label(platform)
                    ),
                );
                closed.push((*platform).to_string())
            }
            Ok(false) => {}
            Err(e) => warnings.push(format!(
                "关闭 {} 失败：{}",
                super::backend::label(platform),
                e.message
            )),
        }
    }
    (closed, warnings)
}

/// 这批会话会牵动哪些**正在运行**的客户端（界面在确认框里点名，别偷偷关）。
pub fn running_clients(keys: &[String]) -> Vec<String> {
    let mut platforms: Vec<&str> = Vec::new();
    for key in keys {
        let Ok(target) = preview::parse_key(key) else {
            continue;
        };
        if let Some(platform) = platform_of(&target.source_key) {
            if !platforms.contains(&platform) {
                platforms.push(platform);
            }
        }
    }
    platforms
        .into_iter()
        .filter(|platform| {
            let names = crate::app::accounts::add::client_process_names(platform);
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            crate::app::process::process_running(&refs)
        })
        .map(super::backend::label)
        .collect()
}

/// 让被关掉的客户端重新拉起，返回告警。
pub fn restart(closed: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();
    restart_clients(closed, &mut warnings);
    warnings
}

fn restart_clients(closed: &[String], warnings: &mut Vec<String>) {
    if closed.is_empty() {
        return;
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    for platform in closed {
        match client_exe(platform) {
            Some(exe) if exe.exists() => match start_detached(&exe) {
                Ok(()) => {
                    // 起来 ≠ 活着：等进程真的出现在列表里，否则「重启」只是自欺欺人
                    let names = crate::app::accounts::add::client_process_names(platform);
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    let mut up = false;
                    for _ in 0..40 {
                        if crate::app::process::process_running(&refs) {
                            up = true;
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                    if up {
                        crate::app::log::log(
                            crate::app::log::Level::Info,
                            "sessions",
                            format!("已重新拉起 {} 客户端", super::backend::label(platform)),
                        );
                    } else {
                        crate::app::log::log(
                            crate::app::log::Level::Warn,
                            "sessions",
                            format!(
                                "{} 客户端重启后没有出现在进程列表里",
                                super::backend::label(platform)
                            ),
                        );
                        warnings.push(format!(
                            "{} 客户端没能自动重启，请手动打开",
                            super::backend::label(platform)
                        ));
                    }
                }
                Err(e) => {
                    crate::app::log::log(
                        crate::app::log::Level::Warn,
                        "sessions",
                        format!(
                            "重启 {} 客户端失败：{}",
                            super::backend::label(platform),
                            e.message
                        ),
                    );
                    warnings.push(format!("重启客户端失败：{}", e.message));
                }
            },
            _ => warnings.push(format!("找不到 {platform} 的安装路径，请手动打开客户端")),
        }
    }
}

/// 主入口：一批行 key，删完返回统计。任何一条失败都不会影响其他条。
/// `with_guard=false` 只给测试用：不碰真实客户端进程。
pub fn purge(roots: &Roots, db: &Db, keys: &[String], with_guard: bool) -> Result<PurgeReport> {
    let mut targets = Vec::new();
    for key in keys {
        targets.push((key.clone(), preview::parse_key(key)?));
    }
    if targets.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "没有选中会话"));
    }

    // 先按来源分组：同一来源只关一次客户端
    let mut platforms: Vec<&str> = Vec::new();
    for (_, target) in &targets {
        if let Some(platform) = platform_of(&target.source_key) {
            if !platforms.contains(&platform) {
                platforms.push(platform);
            }
        }
    }

    let mut report = PurgeReport::default();
    let closed = if with_guard {
        let (closed, warnings) = close_clients(&platforms);
        report.warnings.extend(warnings);
        closed
    } else {
        Vec::new()
    };

    let titles = SqliteSessions::new(db)
        .titles_of(&targets)
        .unwrap_or_default();

    // Trae 是云端占位：远端删不掉，只能在本地记墓碑；索引行保留，靠 hidden 过滤。
    let mut handled: Vec<preview::Target> = Vec::new();
    let mut trae: Vec<String> = Vec::new();
    for (key, target) in &targets {
        if target.kind == "trae" {
            match crate::app::sessions::marks::hide(db, std::slice::from_ref(key)) {
                Ok(_) => {
                    report.deleted += 1;
                    trae.push(key.clone());
                }
                Err(e) => report
                    .errors
                    .push(format!("{}：{}", target.session_id, e.message)),
            }
            continue;
        }
        let title = titles.get(key).cloned().unwrap_or_default();
        match super::backend::backend_for(&target.kind) {
            Some(backend) => match backend.purge(roots, target) {
                Ok((files, bytes)) => {
                    report.deleted += 1;
                    report.files += files;
                    report.bytes += bytes;
                    handled.push((*target).clone());
                }
                Err(e) => report
                    .errors
                    .push(format!("{}：{}", one_line(&title), e.message)),
            },
            None => report.errors.push(format!(
                "{}：{} 的会话删不了（正文在云端）",
                one_line(&title),
                target.kind
            )),
        }
    }

    // 索引行清掉；同一 id 以后重新出现时不该带着旧状态
    let rows: Vec<(String, String, String, String)> = handled
        .iter()
        .map(|t| {
            (
                t.kind.clone(),
                t.source_key.clone(),
                t.account.clone(),
                t.session_id.clone(),
            )
        })
        .collect();
    if !rows.is_empty() {
        if let Err(e) = SqliteSessions::new(db).forget(&rows) {
            crate::app::log::log(
                crate::app::log::Level::Warn,
                "sessions",
                format!("清理会话索引失败：{}", e.message),
            );
        }
        if let Err(e) = crate::app::sessions::marks::forget(
            db,
            &handled
                .iter()
                .map(|t| format!("{}|{}|{}|{}", t.kind, t.source_key, t.account, t.session_id))
                .collect::<Vec<_>>(),
        ) {
            crate::app::log::log(
                crate::app::log::Level::Warn,
                "sessions",
                format!("清理归档标记失败：{}", e.message),
            );
        }
        // 指纹一并作废：下次扫描重读这一源，索引与客户端库重新对齐
        for target in &handled {
            let _ = db.kv_delete(&format!("sess_fp:{}", target.source_key));
        }
    }
    // trae 的隐藏标记要保留：索引行还在，重扫时靠它把这一行挡掉
    let _ = trae;

    if with_guard {
        restart_clients(&closed, &mut report.warnings);
    }
    Ok(report)
}

fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

// ------------------------------------------------------------------ WorkBuddy

pub(crate) fn purge_workbuddy(roots: &Roots, target: &preview::Target) -> Result<(usize, i64)> {
    let ep = wb::editions(roots)
        .into_iter()
        .find(|ep| ep.key == target.source_key)
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "客户端目录不存在"))?;
    let conn = Connection::open_with_flags(ep.db(), OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("打开会话库失败：{e}")))?;
    checkpoint(&conn)?;

    let files = wb::project_files(&ep.projects);
    let body = files.get(&target.session_id).cloned().unwrap_or_default();
    let sidecars = wb::session_sidecars(&ep, &target.session_id);

    let mut count = 0usize;
    let mut size = 0i64;
    for path in &body {
        size += crate::app::fsx::dir_size(path);
        crate::app::fsx::remove_path(path).map_err(|e| {
            AppError::new(
                ErrorKind::Io,
                format!("正文删不掉（{}）：{}", path.display(), e.message),
            )
        })?;
        count += 1;
    }
    for path in &sidecars {
        let bytes = crate::app::fsx::dir_size(path);
        match crate::app::fsx::remove_path(path) {
            Ok(()) => {
                size += bytes;
                count += 1;
            }
            // 侧车被客户端占着（日志最常见）：不值得为它整条失败。
            Err(e) => crate::app::log::log(
                crate::app::log::Level::Warn,
                "sessions",
                format!(
                    "侧车没能删除（{}）：{}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    e.message
                ),
            ),
        }
    }

    // 表可能不存在（客户端版本差异），按「没有这一行」处理
    let _ = conn.execute("DELETE FROM sessions WHERE id = ?1", [&target.session_id]);
    let _ = conn.execute(
        "DELETE FROM session_usage WHERE session_id = ?1",
        [&target.session_id],
    );
    checkpoint(&conn)?;
    Ok((count, size))
}

// ------------------------------------------------------------------ CodeBuddy

pub(crate) fn purge_codebuddy(roots: &Roots, target: &preview::Target) -> Result<(usize, i64)> {
    let history = roots
        .codebuddy()
        .join(&target.account)
        .join(&target.source_key)
        .join(&target.account)
        .join("history");
    let mut found = None;
    if let Ok(entries) = std::fs::read_dir(&history) {
        for entry in entries.flatten() {
            let dir = entry.path().join(&target.session_id);
            if dir.is_dir() {
                found = Some((entry.path(), dir));
                break;
            }
        }
    }
    let Some((ws_dir, session_dir)) = found else {
        // 目录已经不在了：索引残留而已，清索引即可
        return Ok((0, 0));
    };

    let size = crate::app::fsx::dir_size(&session_dir);
    crate::app::fsx::remove_path(&session_dir).map_err(|e| {
        AppError::new(
            ErrorKind::Io,
            format!("会话目录删不掉（{}）：{}", session_dir.display(), e.message),
        )
    })?;

    // 目录已删，现在摘索引；recent.json 不重写也无害，客户端会自己修
    let index_path = ws_dir.join("index.json");
    if let Some(mut value) = read_json(&index_path) {
        if let Some(list) = value
            .get_mut("conversations")
            .and_then(|v| v.as_array_mut())
        {
            list.retain(|item| {
                item.get("id").and_then(|v| v.as_str()) != Some(target.session_id.as_str())
            });
        }
        write_json(&index_path, &value)?;
    }
    let recent_path = ws_dir.join("recent.json");
    if let Some(Value::Array(mut list)) = read_json(&recent_path) {
        let before = list.len();
        list.retain(|item| {
            item.get("id").and_then(|v| v.as_str()) != Some(target.session_id.as_str())
        });
        if list.len() != before {
            write_json(&recent_path, &Value::Array(list))?;
        }
    }

    Ok((1, size))
}

// ------------------------------------------------------------------ Qoder / ZCode

pub(crate) fn purge_sqlite(
    roots: &Roots,
    target: &preview::Target,
    kind: &str,
) -> Result<(usize, i64)> {
    let path = if kind == "qoder" {
        roots
            .qoder(target.source_key == "qoder_intl")
            .join("main.sqlite")
    } else {
        roots.zcode().join("cli").join("db").join("db.sqlite")
    };
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("打开会话库失败：{e}")))?;
    checkpoint(&conn)?;

    // 主表的主键两边不一样：Qoder 是 chat_sessions.session_id，ZCode 是 session.id
    let (main_table, main_key) = if kind == "qoder" {
        ("chat_sessions", "session_id")
    } else {
        ("session", "id")
    };

    // ZCode 的正文在 part 表，按 message_id 关联；先删它，再删 message
    if kind == "zcode" {
        let _ = conn.execute(
            "DELETE FROM part
             WHERE message_id IN (SELECT id FROM message WHERE session_id = ?1)",
            [&target.session_id],
        );
    }
    for table in tables_with_column(&conn, "session_id") {
        if table == main_table {
            continue;
        }
        let _ = conn.execute(
            &format!("DELETE FROM \"{table}\" WHERE session_id = ?1"),
            [&target.session_id],
        );
    }
    let _ = conn.execute(
        &format!("DELETE FROM \"{main_table}\" WHERE {main_key} = ?1"),
        [&target.session_id],
    );
    checkpoint(&conn)?;

    // ZCode 的会话还散落在 `cli/` 下（agents / artifacts / rollout），一并删掉
    let mut count = 0usize;
    let mut size = 0i64;
    if kind == "zcode" {
        for path in
            crate::app::sessions::sources::others::zcode_artifacts(roots, &target.session_id)
        {
            size += crate::app::fsx::dir_size(&path);
            crate::app::fsx::remove_path(&path).map_err(|e| {
                AppError::new(
                    ErrorKind::Io,
                    format!("会话文件删不掉（{}）：{}", path.display(), e.message),
                )
            })?;
            count += 1;
        }
    }
    Ok((count, size))
}

// ------------------------------------------------------------------ SQL 小工具

fn tables_with_column(conn: &Connection, column: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(mut stmt) = conn.prepare("SELECT name FROM sqlite_master WHERE type='table'") else {
        return out;
    };
    let names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    for name in names {
        if name.starts_with("sqlite_") || name.contains("_fts") {
            continue;
        }
        let ok = conn
            .prepare(&format!("PRAGMA table_info(\"{name}\")"))
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get::<_, String>(1))
                    .map(|rows| rows.flatten().any(|c| c == column))
            })
            .unwrap_or(false);
        if ok {
            out.push(name);
        }
    }
    out
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("序列化失败：{e}")))?;
    crate::app::fsx::write_atomic(path, text.as_bytes())
}
