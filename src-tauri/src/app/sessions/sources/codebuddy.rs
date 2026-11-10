//! CodeBuddy（IDE 与 IDEA 插件）的会话。
//!
//! 结构：`%LOCALAPPDATA%\CodeBuddyExtension\Data\<uid>\<平台>\<uid>\history\<工作区>\`
//! 每个工作区一份 `index.json` 列会话，会话正文在同级 `<id>\messages\*.json`。
//! 消息条数就是文件数——不用解析内容。

use crate::app::clients::Roots;
use crate::app::sessions::store::RawRow;
use crate::types::SessionState;
use std::path::{Path, PathBuf};

pub const PLATFORMS: [(&str, &str); 2] = [
    ("CodeBuddyIDE", "CodeBuddy IDE"),
    ("JetBrains", "CodeBuddy 插件"),
];

pub struct CodeSource {
    pub platform: &'static str,
    pub label: &'static str,
    pub fp: String,
    /// (uid, 工作区目录)
    pub dirs: Vec<(String, PathBuf)>,
}

pub fn roots(roots: &Roots) -> Vec<CodeSource> {
    let base = roots.codebuddy();
    PLATFORMS
        .iter()
        .map(|(platform, label)| {
            let mut dirs = Vec::new();
            if let Ok(uid_dirs) = std::fs::read_dir(&base) {
                for uid_dir in uid_dirs.flatten() {
                    if !uid_dir.path().is_dir() {
                        continue;
                    }
                    let platform_dir = uid_dir.path().join(platform);
                    if !platform_dir.is_dir() {
                        continue;
                    }
                    let Ok(accounts) = std::fs::read_dir(&platform_dir) else {
                        continue;
                    };
                    for account in accounts.flatten() {
                        let uid = account.file_name().to_string_lossy().to_string();
                        if !is_uid(&uid) {
                            continue;
                        }
                        let history = account.path().join("history");
                        if history.is_dir() {
                            dirs.push((uid, history));
                        }
                    }
                }
            }
            let fp = fingerprint(&dirs);
            CodeSource {
                platform,
                label,
                fp,
                dirs,
            }
        })
        .collect()
}

/// 工作区索引 + 消息文件数的指纹：客户端续写消息不一定会改 index.json，
/// 只盯索引会漏掉新消息。
fn fingerprint(dirs: &[(String, PathBuf)]) -> String {
    let mut parts = Vec::new();
    for (uid, history) in dirs {
        let Ok(workspaces) = std::fs::read_dir(history) else {
            continue;
        };
        for ws in workspaces.flatten() {
            let ws_path = ws.path();
            if !ws_path.is_dir() {
                continue;
            }
            let index = ws_path.join("index.json");
            let stat = std::fs::metadata(&index).ok();
            parts.push(format!(
                "{uid}/{}:{}:{}",
                ws.file_name().to_string_lossy(),
                stat.as_ref()
                    .and_then(|s| s.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis())
                    .unwrap_or(0),
                stat.map(|s| s.len()).unwrap_or(0)
            ));
            if let Ok(convs) = std::fs::read_dir(&ws_path) {
                for conv in convs.flatten() {
                    if !conv.path().is_dir() {
                        continue;
                    }
                    let messages = conv.path().join("messages");
                    let count = std::fs::read_dir(&messages).map(|d| d.count()).unwrap_or(0);
                    parts.push(format!(
                        "{uid}/{}/{}:{count}",
                        ws.file_name().to_string_lossy(),
                        conv.file_name().to_string_lossy()
                    ));
                }
            }
        }
    }
    parts.sort();
    // v2：把 CodeBuddy JetBrains 的来源标签从「IDEA 插件」改成「CodeBuddy 插件」，
    // 让已有索引强制重扫一次，避免旧标签继续混用。
    format!("v2|{}", parts.join("|"))
}

pub fn scan(src: &CodeSource, errors: &mut Vec<String>) -> Vec<RawRow> {
    let mut out = Vec::new();
    for (uid, history) in &src.dirs {
        let Ok(workspaces) = std::fs::read_dir(history) else {
            continue;
        };
        for ws in workspaces.flatten() {
            let ws_path = ws.path();
            if !ws_path.is_dir() {
                continue;
            }
            let Some(index) = read_json(&ws_path.join("index.json")) else {
                continue;
            };
            let Some(conversations) = index.get("conversations").and_then(|v| v.as_array()) else {
                continue;
            };
            for conv in conversations {
                let Some(id) = conv.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                if id.is_empty() {
                    continue;
                }
                let messages = ws_path.join(id).join("messages");
                let mut msgs = 0i64;
                let mut size = 0i64;
                if let Ok(files) = std::fs::read_dir(&messages) {
                    for file in files.flatten() {
                        if file.path().is_file() {
                            msgs += 1;
                            size += file.metadata().map(|m| m.len() as i64).unwrap_or(0);
                        }
                    }
                }
                let mut row = RawRow::empty("codebuddy", src.platform, src.label);
                row.account = uid.clone();
                row.slug = ws_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                row.session_id = id.to_string();
                row.title = conv
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("（未命名会话）")
                    .to_string();
                row.last_at = iso_ms(
                    conv.get("lastMessageAt")
                        .or_else(|| conv.get("createdAt"))
                        .and_then(|v| v.as_str()),
                );
                row.msgs = msgs;
                row.size = size;
                row.state = SessionState::Visible;
                row.has_body = msgs > 0;
                row.origin = if src.platform == "JetBrains" {
                    "ide-plugin".into()
                } else {
                    "desktop".into()
                };
                out.push(row);
            }
        }
    }
    if out.is_empty() && src.dirs.iter().any(|(_, dir)| dir.is_dir()) {
        errors.push(format!("{}：工作区索引里没有读到会话", src.label));
    }
    out
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// `default` 也当成账号目录：没登录过的 CodeBuddy 用它。
fn is_uid(value: &str) -> bool {
    if value == "default" {
        return true;
    }
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(len, part)| part.len() == *len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

pub fn iso_ms(value: Option<&str>) -> i64 {
    let Some(text) = value else { return 0 };
    if text.is_empty() {
        return 0;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
        return dt.timestamp_millis();
    }
    // 有些版本给的是不带时区的 ISO：按本地时间理解
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f") {
        return dt
            .and_local_timezone(chrono::Local)
            .single()
            .map(|d| d.timestamp_millis())
            .unwrap_or(0);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_shape() {
        assert!(is_uid("8d1f0a2c-1b3c-4d5e-8f90-a1b2c3d4e5f6"));
        assert!(is_uid("default"));
        assert!(!is_uid("history"));
        assert!(!is_uid("8d1f0a2c1b3c4d5e8f90a1b2c3d4e5f6"));
    }

    #[test]
    fn iso_parsing() {
        assert!(iso_ms(Some("2026-10-30T12:00:00Z")) > 1_700_000_000_000);
        assert_eq!(iso_ms(None), 0);
        assert_eq!(iso_ms(Some("")), 0);
    }
}
