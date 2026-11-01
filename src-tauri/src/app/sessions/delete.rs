//! 删除会话：关客户端 → 文件进回收站 → 删库行 → 写清单 → 重启客户端。
//!
//! **顺序不能变**（`docs/phase-4-sessions.md` §6）：文件先进回收站，再删数据库行。
//! 反过来一旦中途失败，就变成「库里没了、文件也找不回」——那才是真的丢数据。
//!
//! 恢复走同一份清单：文件搬回原位、库行原样插回去。删掉的东西都要能回来，
//! 这是这一层存在的全部意义。

use crate::app::accounts::add::{client_exe, process_names};
use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::{AppError, Result};
use crate::app::process::{kill_by_name, process_running, start_detached};
use crate::app::sessions::preview;
use crate::app::sessions::sources::wb;
use crate::app::sessions::store::SqliteSessions;
use crate::app::sessions::trash::{self, Item, Manifest};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::Path;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeleteReport {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub sessions: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub files: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes: i64,
    pub tag: String,
    pub dir: String,
    /// 为写库临时关掉的客户端
    pub closed: Vec<String>,
    /// 关不掉或没重启回来的
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    /// 拼好的中文摘要，界面直接显示
    pub message: String,
}

impl DeleteReport {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "删除 {} 条会话，{} 个文件，{:.1} MB",
            self.sessions,
            self.files,
            self.bytes as f64 / 1_048_576.0
        );
        if !self.closed.is_empty() {
            text.push_str(&format!("，临时关闭 {}", self.closed.join("、")));
        }
        if !self.errors.is_empty() {
            text.push_str(&format!("，{} 条失败", self.errors.len()));
        }
        text
    }
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
/// `restore.rs` 的客户端库写路径共用这道进程闸，所以告警由调用方收编。
pub fn close_clients(platforms: &[&str]) -> (Vec<String>, Vec<String>) {
    let mut closed = Vec::new();
    let mut warnings = Vec::new();
    for platform in platforms {
        let names = process_names(platform);
        if names.is_empty() || !process_running(names) {
            continue;
        }
        for name in names {
            match kill_by_name(name) {
                Ok(0) => {}
                Ok(_) => {
                    if !closed.iter().any(|p: &String| p == platform) {
                        closed.push((*platform).to_string());
                    }
                }
                Err(e) => warnings.push(format!("关闭 {name} 失败：{}", e.message)),
            }
        }
    }
    // 等句柄释放：退出不等于文件立刻可写
    for _ in 0..20 {
        let still = platforms.iter().any(|p| process_running(process_names(p)));
        if !still {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    (closed, warnings)
}

fn close_clients_with_report(platforms: &[&str], report: &mut DeleteReport) -> Vec<String> {
    let (closed, warnings) = close_clients(platforms);
    report.warnings.extend(warnings);
    closed
}

/// 让被关掉的客户端重新拉起（删除与恢复共用），返回告警。
pub fn restart(closed: &[String]) -> Vec<String> {
    let mut report = DeleteReport::default();
    restart_clients(closed, &mut report);
    report.warnings
}

fn restart_clients(closed: &[String], report: &mut DeleteReport) {
    if closed.is_empty() {
        return;
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    for platform in closed {
        match client_exe(platform) {
            Some(exe) if exe.exists() => {
                if let Err(e) = start_detached(&exe) {
                    report
                        .warnings
                        .push(format!("重启客户端失败：{}", e.message));
                }
            }
            _ => report
                .warnings
                .push(format!("找不到 {platform} 的安装路径，请手动打开客户端")),
        }
    }
}

/// 主入口：一批行 key，删完返回统计。任何一条失败都不会影响其他条。
/// `with_guard=false` 只给测试用：不碰真实客户端进程。
pub fn delete(roots: &Roots, db: &Db, keys: &[String], with_guard: bool) -> Result<DeleteReport> {
    delete_into(roots, db, keys, with_guard, &trash::root())
}

/// 回收站根目录可注入：测试用临时目录，绝不碰真实 `data/trash`。
pub fn delete_into(
    roots: &Roots,
    db: &Db,
    keys: &[String],
    with_guard: bool,
    root: &Path,
) -> Result<DeleteReport> {
    let mut targets = Vec::new();
    for key in keys {
        {
            let target = preview::parse_key(key)?;
            targets.push((key.clone(), target))
        }
    }
    if targets.is_empty() {
        return Err(AppError::new("invalid", "没有选中会话"));
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

    std::fs::create_dir_all(root).map_err(|e| AppError::new("io", format!("建回收站失败：{e}")))?;
    let tag = trash::new_tag(root);
    let batch = root.join(&tag);

    let mut report = DeleteReport {
        tag: tag.clone(),
        dir: batch.display().to_string(),
        ..Default::default()
    };
    let closed = if with_guard {
        close_clients_with_report(&platforms, &mut report)
    } else {
        Vec::new()
    };
    report.closed = closed.iter().map(|p| client_label(p)).collect();

    let titles = titles_of(db, &targets);
    let mut items: Vec<Item> = Vec::new();
    // 半成品：文件/导出已经进了回收站、库行没删完的条目。也进清单，
    // 否则这些文件成了恢复界面永远看不到的孤儿，purge 时跟着一起蒸发
    let mut partials: Vec<Item> = Vec::new();
    for (key, target) in &targets {
        let title = titles.get(key).cloned().unwrap_or_default();
        let result = match target.kind.as_str() {
            "workbuddy" => delete_workbuddy(roots, target, &batch, &title, &mut partials),
            "codebuddy" => delete_codebuddy(roots, target, &batch, &title, &mut partials),
            "qoder" => delete_sqlite(roots, target, &batch, &title, "qoder", &mut partials),
            "zcode" => delete_sqlite(roots, target, &batch, &title, "zcode", &mut partials),
            other => Err(AppError::new(
                "unsupported",
                format!("{other} 的会话删不了（正文在云端）"),
            )),
        };
        match result {
            Ok(item) => {
                report.sessions += 1;
                report.files += item.files.len();
                report.bytes += item.size;
                items.push(item);
            }
            Err(e) => report.errors.push(format!("{title}：{}", e.message)),
        }
    }
    for item in &partials {
        report.files += item.files.len();
        report.bytes += item.size;
        report.warnings.push(format!(
            "{} 部分完成：{} 个文件已进回收站，恢复时可原样搬回",
            item.title,
            item.files.len()
        ));
    }
    items.extend(partials);

    if !items.is_empty() {
        let manifest = Manifest {
            tag: tag.clone(),
            at: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            platforms: closed.clone(),
            items,
        };
        trash::write_manifest(&batch, &manifest)?;
        // 索引里这几行已经不存在了：整源失效，下次刷新重建
        let store = SqliteSessions::new(db);
        for (_, target) in &targets {
            let _ = store.drop_source(&target.source_key);
            let _ = db.kv_delete(&format!("sess_fp:{}", target.source_key));
        }
    }

    if with_guard {
        restart_clients(&closed, &mut report);
    }
    report.message = report.summary();
    Ok(report)
}

fn client_label(platform: &str) -> String {
    match platform {
        "workbuddy" => "WorkBuddy 国内版".into(),
        "workbuddy_intl" => "WorkBuddy 国际版".into(),
        "qoder" => "Qoder CN".into(),
        "qoder_intl" => "Qoder 国际版".into(),
        "zcode" => "ZCode".into(),
        other => other.to_string(),
    }
}

fn titles_of(
    db: &Db,
    targets: &[(String, preview::Target)],
) -> std::collections::HashMap<String, String> {
    // 一条连接循环查：每个 key 各开一次事务太浪费
    db.read(|conn| {
        let mut out = std::collections::HashMap::new();
        let mut stmt = match conn.prepare(
            "SELECT title FROM session_index
             WHERE kind = ?1 AND source_key = ?2 AND account = ?3 AND session_id = ?4",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Ok(out),
        };
        for (key, target) in targets {
            let title: Option<String> = stmt
                .query_row(
                    rusqlite::params![
                        target.kind,
                        target.source_key,
                        target.account,
                        target.session_id
                    ],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            if let Some(title) = title {
                out.insert(key.clone(), title);
            }
        }
        Ok(out)
    })
    .unwrap_or_default()
}

// ------------------------------------------------------------------ WorkBuddy

/// 库行删一半失败时的兜底：文件已进回收站，把半成品记进清单。
/// 行数据一并带上——恢复走 INSERT OR REPLACE，幂等，能原样搬回。
fn salvage_workbuddy(
    partial: &mut Vec<Item>,
    target: &preview::Target,
    title: &str,
    moved: &[String],
    size: i64,
    session: &Option<Value>,
    usage: &[Value],
) {
    if moved.is_empty() {
        return;
    }
    partial.push(Item {
        kind: "workbuddy".into(),
        source: target.source_key.clone(),
        session_id: target.session_id.clone(),
        title: title.to_string(),
        files: moved.to_vec(),
        size,
        rows: Some(serde_json::json!({
            "sessions": session,
            "session_usage": usage,
        })),
        account: target.account.clone(),
    });
}

fn delete_workbuddy(
    roots: &Roots,
    target: &preview::Target,
    batch: &Path,
    title: &str,
    partial: &mut Vec<Item>,
) -> Result<Item> {
    let ep = wb::editions(roots)
        .into_iter()
        .find(|ep| ep.key == target.source_key)
        .ok_or_else(|| AppError::new("invalid", "客户端目录不存在"))?;
    let db = ep.db();
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| AppError::new("sqlite", format!("打开会话库失败：{e}")))?;
    checkpoint(&conn)?;

    let session = dump_row(&conn, "sessions", "id", &target.session_id)?;
    let usage = dump_rows(&conn, "session_usage", "session_id", &target.session_id)?;
    if session.is_none() && usage.is_empty() {
        return Err(AppError::new("invalid", "库里已经找不到这条会话"));
    }

    // 文件先搬走：搬不动就不删库行
    let files = wb::project_files(&ep.projects);
    let owned = files.get(&target.session_id).cloned().unwrap_or_default();
    let mut moved = Vec::new();
    let mut size = 0;
    for path in &owned {
        let rel = path
            .strip_prefix(&ep.root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });
        let rel = format!("{}/{}", ep.key, rel);
        size += trash::dir_size(path);
        match trash::move_in(path, batch, &rel) {
            Ok(()) => moved.push(rel),
            Err(e) => {
                return Err(AppError::new(
                    "io",
                    format!("正文没能移进回收站：{}", e.message),
                ))
            }
        }
    }

    if session.is_some() {
        conn.execute("DELETE FROM sessions WHERE id = ?1", [&target.session_id])
            .map_err(|e| {
                salvage_workbuddy(partial, target, title, &moved, size, &session, &usage);
                AppError::new("sqlite", format!("删除会话行失败：{e}"))
            })?;
    }
    if !usage.is_empty() {
        conn.execute(
            "DELETE FROM session_usage WHERE session_id = ?1",
            [&target.session_id],
        )
        .map_err(|e| {
            salvage_workbuddy(partial, target, title, &moved, size, &session, &usage);
            AppError::new("sqlite", format!("删除用量行失败：{e}"))
        })?;
    }
    if let Err(e) = checkpoint(&conn) {
        salvage_workbuddy(partial, target, title, &moved, size, &session, &usage);
        return Err(e);
    }

    let rows = serde_json::json!({ "sessions": session, "session_usage": usage });
    Ok(Item {
        kind: "workbuddy".into(),
        source: target.source_key.clone(),
        session_id: target.session_id.clone(),
        title: title.to_string(),
        files: moved,
        size,
        rows: Some(rows),
        account: target.account.clone(),
    })
}

// ------------------------------------------------------------------ CodeBuddy

fn delete_codebuddy(
    roots: &Roots,
    target: &preview::Target,
    batch: &Path,
    title: &str,
    partial: &mut Vec<Item>,
) -> Result<Item> {
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
        return Err(AppError::new("invalid", "会话目录已经不在了"));
    };

    let index_path = ws_dir.join("index.json");
    let original = read_json(&index_path).and_then(|value| {
        value
            .get("conversations")
            .and_then(|v| v.as_array())
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| {
                        item.get("id").and_then(|v| v.as_str()) == Some(target.session_id.as_str())
                    })
                    .cloned()
            })
    });

    // 先搬目录再摘索引：搬失败时索引原样，客户端索引与磁盘保持一致；
    // 反过来搬失败会留下「索引有条目、目录已消失」的不一致状态
    let size = trash::dir_size(&session_dir);
    let rel = format!(
        "codebuddy/{}/{}/{}/{}",
        target.source_key,
        target.account,
        ws_dir.file_name().unwrap_or_default().to_string_lossy(),
        target.session_id
    );
    trash::move_in(&session_dir, batch, &rel)?;

    // 目录已进回收站，现在摘索引；摘失败时把半成品记进清单（可恢复）
    let salvage = |partial: &mut Vec<Item>, e: AppError| {
        partial.push(Item {
            kind: "codebuddy".into(),
            source: target.source_key.clone(),
            session_id: target.session_id.clone(),
            title: title.to_string(),
            files: vec![rel.clone()],
            size,
            rows: Some(serde_json::json!({
                "workspace": ws_dir.file_name().unwrap_or_default().to_string_lossy(),
                "conversation": original.clone(),
            })),
            account: target.account.clone(),
        });
        e
    };
    if let Some(mut value) = read_json(&index_path) {
        if let Some(list) = value
            .get_mut("conversations")
            .and_then(|v| v.as_array_mut())
        {
            list.retain(|item| {
                item.get("id").and_then(|v| v.as_str()) != Some(target.session_id.as_str())
            });
        }
        write_json(&index_path, &value)
            .map_err(|e| salvage(partial, e))?;
    }
    let recent_path = ws_dir.join("recent.json");
    if let Some(Value::Array(mut list)) = read_json(&recent_path) {
        let before = list.len();
        list.retain(|item| {
            item.get("id").and_then(|v| v.as_str()) != Some(target.session_id.as_str())
        });
        if list.len() != before {
            write_json(&recent_path, &Value::Array(list))
                .map_err(|e| salvage(partial, e))?;
        }
    }

    Ok(Item {
        kind: "codebuddy".into(),
        source: target.source_key.clone(),
        session_id: target.session_id.clone(),
        title: title.to_string(),
        files: vec![rel],
        size,
        rows: Some(serde_json::json!({
            "workspace": ws_dir.file_name().unwrap_or_default().to_string_lossy(),
            "conversation": original,
        })),
        account: target.account.clone(),
    })
}

// ------------------------------------------------------------------ Qoder / ZCode

/// 把会话相关的所有行（带 session_id 的表）导出成 JSON，然后删掉。
fn delete_sqlite(
    roots: &Roots,
    target: &preview::Target,
    batch: &Path,
    title: &str,
    kind: &str,
    partial: &mut Vec<Item>,
) -> Result<Item> {
    let path = if kind == "qoder" {
        roots
            .qoder(target.source_key == "qoder_intl")
            .join("main.sqlite")
    } else {
        roots.zcode().join("cli").join("db").join("db.sqlite")
    };
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| AppError::new("sqlite", format!("打开会话库失败：{e}")))?;
    checkpoint(&conn)?;

    let tables = tables_with_column(&conn, "session_id");
    let mut dumped = Map::new();
    // ZCode 的正文在 part 表，它按 message_id 关联、没有 session_id，
    // 不一起搬走会留下孤儿行：库里越积越大，恢复时正文也没了
    let parts = if kind == "zcode" {
        dump_parts(&conn, &target.session_id)?
    } else {
        Vec::new()
    };
    // 主表的主键两边不一样：Qoder 是 chat_sessions.session_id，ZCode 是 session.id
    let (main_table, main_key) = if kind == "qoder" {
        ("chat_sessions", "session_id")
    } else {
        ("session", "id")
    };
    if let Some(session) = dump_row(&conn, main_table, main_key, &target.session_id)? {
        dumped.insert(main_table.into(), Value::Array(vec![session]));
    }
    for table in &tables {
        let rows = dump_rows(&conn, table, "session_id", &target.session_id)?;
        if !rows.is_empty() {
            dumped.insert(table.clone(), Value::Array(rows));
        }
    }
    if parts.is_empty() && dumped.is_empty() {
        return Err(AppError::new("invalid", "库里已经找不到这条会话"));
    }
    if !parts.is_empty() {
        dumped.insert("part".into(), Value::Array(parts.clone()));
    }

    // 没有正文文件可搬，把这段 JSON 写进批次目录，恢复时读它
    let rel = format!("{}/{}/{}.json", kind, target.source_key, target.session_id);
    let text = serde_json::to_string_pretty(&Value::Object(dumped.clone()))
        .map_err(|e| AppError::new("io", format!("导出失败：{e}")))?;
    let dest = batch.join(&rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::new("io", format!("建目录失败：{e}")))?;
    }
    std::fs::write(&dest, text.as_bytes())
        .map_err(|e| AppError::new("io", format!("写回收站失败：{e}")))?;
    let size = dest.metadata().map(|m| m.len() as i64).unwrap_or(0);

    // 导出 JSON 已落进批次目录：从这里开始任何一步失败都要把它记进清单，
    // 否则它成了恢复界面看不到的孤儿
    let salvage = |partial: &mut Vec<Item>, e: AppError| {
        partial.push(Item {
            kind: kind.into(),
            source: target.source_key.clone(),
            session_id: target.session_id.clone(),
            title: title.to_string(),
            files: vec![rel.clone()],
            size,
            rows: Some(Value::Object(dumped.clone())),
            account: target.account.clone(),
        });
        e
    };
    for part in &parts {
        if let Some(id) = part.get("id").and_then(|v| v.as_str()) {
            conn.execute("DELETE FROM part WHERE id = ?1", [id])
                .map_err(|e| salvage(partial, AppError::new("sqlite", format!("删除消息片段失败：{e}"))))?;
        }
    }
    for table in &tables {
        conn.execute(
            &format!("DELETE FROM \"{table}\" WHERE session_id = ?1"),
            [&target.session_id],
        )
        .map_err(|e| salvage(partial, AppError::new("sqlite", format!("删除 {table} 失败：{e}"))))?;
    }
    conn.execute(
        &format!("DELETE FROM \"{main_table}\" WHERE {main_key} = ?1"),
        [&target.session_id],
    )
    .map_err(|e| salvage(partial, AppError::new("sqlite", format!("删除会话行失败：{e}"))))?;
    if let Err(e) = checkpoint(&conn) {
        return Err(salvage(partial, e));
    }

    Ok(Item {
        kind: kind.into(),
        source: target.source_key.clone(),
        session_id: target.session_id.clone(),
        title: title.to_string(),
        files: vec![rel],
        size,
        rows: Some(Value::Object(dumped)),
        account: target.account.clone(),
    })
}

// ------------------------------------------------------------------ 恢复 / 清空

/// 清单是本地文件，但一份被截断/手改过的 manifest 不该能把文件写穿客户端目录：
/// 相对路径里出现 `..` 直接拒绝。
fn check_rel(rel: &str) -> Result<()> {
    if rel.split('/').any(|seg| seg == "..") {
        return Err(AppError::new("invalid", "清单里的路径不合法"));
    }
    Ok(())
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct RestoreReport {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub restored: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn restore(roots: &Roots, tag: &str, with_guard: bool) -> Result<RestoreReport> {
    restore_from(roots, tag, with_guard, &trash::root())
}

pub fn restore_from(
    roots: &Roots,
    tag: &str,
    with_guard: bool,
    root: &Path,
) -> Result<RestoreReport> {
    let dir = root.join(tag);
    let manifest = trash::read_manifest(&dir)
        .ok_or_else(|| AppError::new("invalid", format!("找不到批次 {tag} 的清单")))?;

    let mut report = RestoreReport::default();
    let mut delete_report = DeleteReport::default();
    let platforms: Vec<&str> = manifest.platforms.iter().map(|p| p.as_str()).collect();
    let closed = if with_guard {
        close_clients_with_report(&platforms, &mut delete_report)
    } else {
        Vec::new()
    };
    report.warnings.append(&mut delete_report.warnings);

    for item in &manifest.items {
        let result = match item.kind.as_str() {
            "workbuddy" => restore_workbuddy(roots, &dir, item),
            "codebuddy" => restore_codebuddy(roots, &dir, item),
            "qoder" => restore_sqlite(roots, item, "qoder"),
            "zcode" => restore_sqlite(roots, item, "zcode"),
            other => Err(AppError::new(
                "unsupported",
                format!("不认识的类型：{other}"),
            )),
        };
        match result {
            Ok(()) => report.restored += 1,
            Err(e) => report.errors.push(format!("{}：{}", item.title, e.message)),
        }
    }

    if with_guard {
        restart_clients(&closed, &mut delete_report);
    }
    report.warnings.append(&mut delete_report.warnings);
    // 全部恢复成功就把批次目录清掉，免得再点一次重复插行
    if report.errors.is_empty() {
        let _ = crate::app::fsx::remove_path(&dir);
    }
    Ok(report)
}

fn restore_workbuddy(roots: &Roots, dir: &Path, item: &Item) -> Result<()> {
    let ep = wb::editions(roots)
        .into_iter()
        .find(|ep| ep.key == item.source)
        .ok_or_else(|| AppError::new("invalid", "客户端目录不存在"))?;
    let conn = Connection::open_with_flags(ep.db(), OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| AppError::new("sqlite", format!("打开会话库失败：{e}")))?;
    checkpoint(&conn)?;

    if let Some(rows) = item.rows.as_ref() {
        if let Some(session) = rows.get("sessions").and_then(|v| v.as_object()) {
            insert_json_row(&conn, "sessions", session)?;
        }
        if let Some(Value::Array(usage)) = rows.get("session_usage") {
            for row in usage {
                if let Some(map) = row.as_object() {
                    insert_json_row(&conn, "session_usage", map)?;
                }
            }
        }
    }
    for rel in &item.files {
        check_rel(rel)?;
        // 批次里的相对路径形如 `cn/projects/<slug>/<sid>.jsonl`
        let dest = rel
            .split_once('/')
            .map(|(_, rest)| ep.root.join(rest))
            .ok_or_else(|| AppError::new("invalid", "清单里的路径不对"))?;
        trash::move_out(dir, rel, &dest)?;
    }
    checkpoint(&conn)?;
    Ok(())
}

fn restore_codebuddy(roots: &Roots, dir: &Path, item: &Item) -> Result<()> {
    check_rel(&item.session_id)?;
    let workspace = item
        .rows
        .as_ref()
        .and_then(|v| v.get("workspace"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    check_rel(workspace)?;
    let base = roots
        .codebuddy()
        .join(&item.account)
        .join(&item.source)
        .join(&item.account)
        .join("history")
        .join(workspace);
    let rel = item
        .files
        .first()
        .ok_or_else(|| AppError::new("invalid", "清单里没有文件"))?;
    let dest = base.join(&item.session_id);
    trash::move_out(dir, rel, &dest)?;

    // 索引里补回去；recent.json 不补（客户端自己会写）
    let index_path = base.join("index.json");
    if let Some(conversation) = item
        .rows
        .as_ref()
        .and_then(|v| v.get("conversation"))
        .cloned()
    {
        let mut value =
            read_json(&index_path).unwrap_or_else(|| serde_json::json!({"conversations": []}));
        if let Some(list) = value
            .get_mut("conversations")
            .and_then(|v| v.as_array_mut())
        {
            let exists = list
                .iter()
                .any(|x| x.get("id").and_then(|v| v.as_str()) == Some(item.session_id.as_str()));
            if !exists {
                list.push(conversation);
            }
        }
        write_json(&index_path, &value)?;
    }
    Ok(())
}

fn restore_sqlite(roots: &Roots, item: &Item, kind: &str) -> Result<()> {
    let path = if kind == "qoder" {
        roots.qoder(item.source == "qoder_intl").join("main.sqlite")
    } else {
        roots.zcode().join("cli").join("db").join("db.sqlite")
    };
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| AppError::new("sqlite", format!("打开会话库失败：{e}")))?;
    checkpoint(&conn)?;

    let Some(Value::Object(tables)) = item.rows.as_ref() else {
        return Err(AppError::new("invalid", "清单里没有导出的行"));
    };
    // 主表先插，其余表后面跟着
    let mut order: Vec<&String> = tables.keys().collect();
    order.sort_by_key(|name| {
        if name.as_str() == "chat_sessions" || name.as_str() == "session" {
            0
        } else {
            1
        }
    });
    for table in order {
        let Some(list) = tables[table].as_array() else {
            continue;
        };
        for row in list {
            if let Some(map) = row.as_object() {
                insert_json_row(&conn, table, map)?;
            }
        }
    }
    checkpoint(&conn)?;
    Ok(())
}

pub fn purge(tag: &str) -> Result<i64> {
    trash::purge(&trash::root(), tag)
}

pub fn list() -> Vec<Manifest> {
    trash::list(&trash::root())
}

pub fn list_in(root: &Path) -> Vec<Manifest> {
    trash::list(root)
}

// ------------------------------------------------------------------ SQL 小工具

/// 写前清 WAL：客户端读不到没 checkpoint 的改动，这条在四条写路径上都要走。
pub fn checkpoint(conn: &Connection) -> Result<()> {
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(()),
            other => Err(AppError::new(
                "sqlite",
                format!("wal_checkpoint 失败：{other}"),
            )),
        })
}

fn dump_row(conn: &Connection, table: &str, key: &str, value: &str) -> Result<Option<Value>> {
    Ok(dump_rows(conn, table, key, value)?.into_iter().next())
}

fn dump_rows(conn: &Connection, table: &str, key: &str, value: &str) -> Result<Vec<Value>> {
    let sql = format!("SELECT * FROM \"{table}\" WHERE {key} = ?1");
    let mut stmt = match conn.prepare(&sql) {
        Ok(stmt) => stmt,
        // 表不存在（客户端版本差异）不算错误
        Err(_) => return Ok(Vec::new()),
    };
    let names: Vec<String> = stmt.column_names().iter().map(|n| n.to_string()).collect();
    let mut rows = stmt
        .query([value])
        .map_err(|e| AppError::new("sqlite", format!("查询 {table} 失败：{e}")))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| AppError::new("sqlite", format!("读 {table} 失败：{e}")))?
    {
        let mut map = Map::new();
        for (index, name) in names.iter().enumerate() {
            map.insert(name.clone(), sql_to_json(row, index));
        }
        out.push(Value::Object(map));
    }
    Ok(out)
}

fn sql_to_json(row: &rusqlite::Row<'_>, index: usize) -> Value {
    match row.get_ref(index) {
        Ok(rusqlite::types::ValueRef::Null) => Value::Null,
        Ok(rusqlite::types::ValueRef::Integer(n)) => Value::from(n),
        Ok(rusqlite::types::ValueRef::Real(f)) => Value::from(f),
        Ok(rusqlite::types::ValueRef::Text(bytes)) => {
            Value::String(String::from_utf8_lossy(bytes).to_string())
        }
        Ok(rusqlite::types::ValueRef::Blob(bytes)) => {
            // BLOB 用带标记的对象编码：恢复时才能区分「BLOB」和「恰好长得像的文本」
            use base64::Engine;
            serde_json::json!({
                "$blob": base64::engine::general_purpose::STANDARD.encode(bytes)
            })
        }
        Err(_) => Value::Null,
    }
}

/// ZCode 的消息片段：`part.message_id` 指向 `message.id`，按会话下的消息反查。
fn dump_parts(conn: &Connection, session_id: &str) -> Result<Vec<Value>> {
    let mut stmt = match conn.prepare(
        "SELECT p.* FROM part p JOIN message m ON p.message_id = m.id WHERE m.session_id = ?1",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Ok(Vec::new()),
    };
    let names: Vec<String> = stmt.column_names().iter().map(|n| n.to_string()).collect();
    let mut rows = stmt
        .query([session_id])
        .map_err(|e| AppError::new("sqlite", format!("查询 part 失败：{e}")))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| AppError::new("sqlite", format!("读 part 失败：{e}")))?
    {
        let mut map = Map::new();
        for (index, name) in names.iter().enumerate() {
            map.insert(name.clone(), sql_to_json(row, index));
        }
        out.push(Value::Object(map));
    }
    Ok(out)
}

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

/// 按目标库的实际列插入，行里多出来的列直接跳过（客户端版本差异）。
fn insert_json_row(conn: &Connection, table: &str, row: &Map<String, Value>) -> Result<()> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(|e| AppError::new("sqlite", format!("读 {table} 结构失败：{e}")))?;
    let columns: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    let names: Vec<&String> = columns
        .iter()
        // FTS 外挂表的 row_id 由 SQLite 重新分配，硬插会和现有行撞号
        .filter(|name| {
            row.contains_key(*name)
                && !(table.contains("search_segments") && name.as_str() == "row_id")
        })
        .collect();
    if names.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; names.len()].join(",");
    let sql = format!(
        "INSERT OR REPLACE INTO \"{table}\" ({}) VALUES ({placeholders})",
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let values: Vec<rusqlite::types::Value> = names
        .iter()
        .map(|name| json_to_sql(&row[*name]))
        .collect::<Result<Vec<_>>>()?;
    conn.execute(&sql, rusqlite::params_from_iter(values))
        .map_err(|e| AppError::new("sqlite", format!("恢复 {table} 行失败：{e}")))?;
    Ok(())
}

fn json_to_sql(value: &Value) -> Result<rusqlite::types::Value> {
    use rusqlite::types::Value as Sql;
    Ok(match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Sql::Integer(i)
            } else {
                Sql::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(text) => Sql::Text(text.clone()),
        // dump 时的 BLOB 标记：还原成二进制，而不是当普通 JSON 文本存进去
        Value::Object(map) if map.len() == 1 => {
            use base64::Engine;
            match map.get("$blob").and_then(|v| v.as_str()) {
                Some(text) => Sql::Blob(
                    base64::engine::general_purpose::STANDARD
                        .decode(text)
                        .map_err(|e| {
                            AppError::new("invalid", format!("清单里的 BLOB 不是合法 base64：{e}"))
                        })?,
                ),
                None => Sql::Text(value.to_string()),
            }
        }
        Value::Array(_) | Value::Object(_) => Sql::Text(value.to_string()),
    })
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| AppError::new("io", format!("序列化失败：{e}")))?;
    crate::app::fsx::write_atomic(path, text.as_bytes())
}
