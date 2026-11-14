//! Trae CN / 国际版的会话。
//!
//! 旧版把会话写在 `User/**/state.vscdb` 的 `ItemTable` 里，key 含
//! `icube-ai-agent`；新版 `ModularData/ai-agent/database.db` 是 SQLCipher
//! 加密库，读不到时只能明确报错，不能返回空列表冒充「没有会话」。

use crate::app::clients::Roots;
use crate::app::sessions::sources::SourceInfo;
use crate::app::sessions::store::{RawRow, UNNAMED_TITLE};
use crate::types::SessionState;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const EDITIONS: [(&str, &str, bool); 2] =
    [("Trae CN", "Trae CN", true), ("Trae", "Trae 国际版", false)];

pub fn sources(roots: &Roots) -> Vec<SourceInfo> {
    EDITIONS
        .iter()
        .map(|(key, label, cn)| {
            let root = roots.trae(*cn);
            SourceInfo {
                key: (*key).into(),
                label: (*label).into(),
                fp: fingerprint(&root),
                present: root.is_dir(),
            }
        })
        .collect()
}

fn fingerprint(root: &Path) -> String {
    let mut parts = Vec::new();
    for db in state_dbs(root) {
        if let Ok(stat) = std::fs::metadata(&db) {
            parts.push(format!("db:{}:{}", mtime_ms(&stat), stat.len()));
        }
    }
    let modular = root
        .join("ModularData")
        .join("ai-agent")
        .join("database.db");
    if let Ok(stat) = std::fs::metadata(&modular) {
        parts.push(format!("modular:{}:{}", mtime_ms(&stat), stat.len()));
    }
    if parts.is_empty() {
        "absent".into()
    } else {
        parts.sort();
        parts.join("|")
    }
}

fn mtime_ms(stat: &std::fs::Metadata) -> u128 {
    stat.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn state_dbs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let global = root.join("User").join("globalStorage").join("state.vscdb");
    if global.exists() {
        out.push(global);
    }
    let workspace = root.join("User").join("workspaceStorage");
    if let Ok(dirs) = std::fs::read_dir(&workspace) {
        for dir in dirs.flatten() {
            let db = dir.path().join("state.vscdb");
            if db.exists() {
                out.push(db);
            }
        }
    }
    out
}

pub fn scan(roots: &Roots, key: &str, label: &str, errors: &mut Vec<String>) -> Vec<RawRow> {
    let root = roots.trae(key == "Trae CN");
    let mut by_id: HashMap<String, RawRow> = HashMap::new();

    for db in state_dbs(&root) {
        match read_db(&db, key, label) {
            Ok(rows) => {
                for row in rows {
                    match by_id.get(&row.session_id) {
                        Some(old) if old.last_at >= row.last_at => {}
                        _ => {
                            by_id.insert(row.session_id.clone(), row);
                        }
                    }
                }
            }
            Err(e) => errors.push(format!(
                "{label}：{} 读取失败（{e}）",
                db.file_name().unwrap_or_default().to_string_lossy()
            )),
        }
    }

    if by_id.is_empty() {
        let modular = root
            .join("ModularData")
            .join("ai-agent")
            .join("database.db");
        if modular.exists() {
            errors.push(format!(
                "{label}：新版会话库为加密库（SQLCipher），当前只能读取旧版 state.vscdb；请在 Trae 中导出会话或保留旧版数据"
            ));
        }
    }

    let mut rows: Vec<RawRow> = by_id.into_values().collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.last_at));
    rows
}

fn read_db(path: &Path, key: &str, label: &str) -> Result<Vec<RawRow>, String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=1500;")
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key LIKE ?")
        .map_err(|e| e.to_string())?;
    let iter = stmt
        .query_map(["%icube-ai-agent%"], |row| row.get::<_, SqlValue>(0))
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for value in iter.flatten() {
        let bytes = match value {
            SqlValue::Blob(bytes) => bytes,
            SqlValue::Text(text) => text.into_bytes(),
            _ => continue,
        };
        let Some(root) = serde_json::from_slice::<Value>(&bytes).ok() else {
            continue;
        };
        let list = root
            .get("list")
            .and_then(Value::as_array)
            .or_else(|| root.as_array());
        let Some(list) = list else { continue };
        for item in list {
            if let Some(row) = parse_session(item, key, label) {
                out.push(row);
            }
        }
    }
    Ok(out)
}

fn parse_session(item: &Value, key: &str, label: &str) -> Option<RawRow> {
    let session_id = item
        .get("sessionId")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)?;
    if session_id.is_empty() {
        return None;
    }

    let messages = item.get("messages").and_then(Value::as_array);
    let msgs = messages.map(Vec::len).unwrap_or(0) as i64;
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| messages.and_then(|items| items.iter().find_map(first_user_text)))
        .unwrap_or_else(|| UNNAMED_TITLE.into());
    let updated = item
        .get("updatedAt")
        .or_else(|| item.get("createdAt"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let size = serde_json::to_vec(item)
        .map(|bytes| bytes.len() as i64)
        .unwrap_or(0);

    let mut row = RawRow::empty("trae", key, label);
    row.session_id = session_id.to_string();
    row.title = title;
    row.last_at = normalize_ms(updated);
    row.msgs = msgs;
    row.size = size;
    row.state = SessionState::Visible;
    row.has_body = msgs > 0;
    row.origin = "desktop".into();
    Some(row)
}

fn first_user_text(item: &Value) -> Option<String> {
    if item.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let text = message_text(item);
    (!text.is_empty()).then_some(text)
}

pub(crate) fn message_text(item: &Value) -> String {
    if let Some(content) = item.get("content") {
        if let Some(text) = content.as_str() {
            if !text.is_empty() {
                return text.to_string();
            }
        }
        if let Some(parts) = content.as_array() {
            let joined = parts
                .iter()
                .filter_map(|part| {
                    if let Some(text) = part.as_str() {
                        return Some(text.to_string());
                    }
                    let obj = part.as_object()?;
                    if obj.get("type").and_then(Value::as_str) == Some("text") {
                        obj.get("text").and_then(Value::as_str).map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !joined.is_empty() {
                return joined;
            }
        }
    }
    let task = item.get("agentTaskContent");
    if let Some(proposal) = task
        .and_then(|value| value.get("proposal"))
        .and_then(Value::as_str)
    {
        if !proposal.is_empty() {
            return proposal.to_string();
        }
    }
    task.and_then(|value| value.get("guideline"))
        .and_then(|value| value.get("planItems"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("thought").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

pub(crate) fn preview(
    roots: &Roots,
    key: &str,
    session_id: &str,
) -> Option<Vec<(String, String, i64)>> {
    let root = roots.trae(key == "Trae CN");
    for db in state_dbs(&root) {
        if let Some(messages) = read_messages(&db, session_id) {
            return Some(messages);
        }
    }
    None
}

fn read_messages(path: &Path, session_id: &str) -> Option<Vec<(String, String, i64)>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=1500;")
        .ok()?;
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key LIKE ?")
        .ok()?;
    let iter = stmt
        .query_map(["%icube-ai-agent%"], |row| row.get::<_, SqlValue>(0))
        .ok()?;

    for value in iter.flatten() {
        let bytes = match value {
            SqlValue::Blob(bytes) => bytes,
            SqlValue::Text(text) => text.into_bytes(),
            _ => continue,
        };
        let Ok(root) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(list) = root
            .get("list")
            .and_then(Value::as_array)
            .or_else(|| root.as_array())
        else {
            continue;
        };
        let Some(item) = list.iter().find(|item| {
            item.get("sessionId")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                == Some(session_id)
        }) else {
            continue;
        };

        let mut out = Vec::new();
        for message in item
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(role) = message.get("role").and_then(Value::as_str) else {
                continue;
            };
            if role != "user" && role != "assistant" {
                continue;
            }
            let text = message_text(message);
            if text.is_empty() {
                continue;
            }
            let time = message
                .get("timestamp")
                .or_else(|| message.get("createdAt"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            out.push((role.to_string(), text, normalize_ms(time)));
        }
        return Some(out);
    }
    None
}

fn normalize_ms(value: i64) -> i64 {
    if value > 0 && value < 1_000_000_000_000 {
        value * 1000
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use tempfile::TempDir;

    fn roots(dir: &TempDir) -> Roots {
        Roots::at(
            dir.path().join("home"),
            dir.path().join("appdata"),
            dir.path().join("local"),
        )
    }

    fn seed_state_db(root: &Path) {
        let global = root.join("User").join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        let conn = Connection::open(global.join("state.vscdb")).unwrap();
        conn.execute_batch("CREATE TABLE ItemTable(key TEXT PRIMARY KEY, value BLOB);")
            .unwrap();
        let value = serde_json::json!({
            "list": [{
                "sessionId": "session-1",
                "title": "Trae 本地会话",
                "updatedAt": 1_760_000_000,
                "messages": [
                    {"role": "user", "content": "你好"},
                    {"role": "assistant", "content": "在", "timestamp": 1_760_000_001}
                ]
            }]
        });
        conn.execute(
            "INSERT INTO ItemTable VALUES (?1, ?2)",
            params![
                "memento/icube-ai-agent-storage",
                serde_json::to_vec(&value).unwrap()
            ],
        )
        .unwrap();
    }

    #[test]
    fn scans_legacy_state_db_and_preview() {
        let dir = TempDir::new().unwrap();
        let roots = roots(&dir);
        seed_state_db(&roots.trae(true));

        let mut errors = Vec::new();
        let rows = scan(&roots, "Trae CN", "Trae CN", &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "session-1");
        assert_eq!(rows[0].title, "Trae 本地会话");
        assert_eq!(rows[0].msgs, 2);
        assert_eq!(rows[0].last_at, 1_760_000_000_000);
        assert_eq!(rows[0].state, SessionState::Visible);

        let messages = preview(&roots, "Trae CN", "session-1").unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].0, "user");
        assert_eq!(messages[0].1, "你好");
        assert_eq!(messages[1].0, "assistant");
    }

    #[test]
    fn encrypted_modular_db_is_reported_not_empty_silently() {
        let dir = TempDir::new().unwrap();
        let roots = roots(&dir);
        let modular = roots
            .trae(true)
            .join("ModularData")
            .join("ai-agent")
            .join("database.db");
        std::fs::create_dir_all(modular.parent().unwrap()).unwrap();
        std::fs::write(modular, b"not a plain sqlite").unwrap();

        let mut errors = Vec::new();
        let rows = scan(&roots, "Trae CN", "Trae CN", &mut errors);
        assert!(rows.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("加密库"), "{errors:?}");
    }
}
