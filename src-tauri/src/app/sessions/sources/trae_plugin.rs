//! Trae / MarsCode IDEA 插件的会话。
//!
//! 插件主库 `~/.marscode/ai-chat/AppData/IDEA/ai-agent/database.db` 是 SQLCipher 加密库，
//! 不能直接读。这里改用插件自己的运行日志：`lite/get_messages response` 里带完整
//! 用户/助手消息，足以还原标题、时间、消息数与正文预览。
//! 这是「本机落盘数据」，不是官方导出，但比把加密库显示成空列表更接近事实。

use crate::app::clients::Roots;
use crate::app::sessions::sources::SourceInfo;
use crate::app::sessions::store::RawRow;
use crate::types::SessionState;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const EDITIONS: [(&str, &str); 1] = [("trae_plugin_cn", "Trae CN 插件")];

#[derive(Debug, Clone)]
struct Message {
    index: i64,
    role: String,
    text: String,
    time: i64,
}

#[derive(Default)]
struct Session {
    messages: BTreeMap<String, Message>,
    last_at: i64,
    size: i64,
}

pub fn sources(roots: &Roots) -> Vec<SourceInfo> {
    EDITIONS
        .iter()
        .map(|(key, label)| {
            let root = roots.trae_plugin();
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
    for file in log_files(root) {
        if let Ok(stat) = std::fs::metadata(&file) {
            parts.push(format!(
                "{}:{}:{}",
                file.to_string_lossy(),
                mtime_ms(&stat),
                stat.len()
            ));
        }
    }
    if parts.is_empty() {
        "absent".into()
    } else {
        parts.sort();
        parts.join("|")
    }
}

fn log_files(root: &Path) -> Vec<PathBuf> {
    let logs = root.join("logs");
    let mut out = Vec::new();
    walk_logs(&logs, &mut out);
    out.sort();
    out
}

fn walk_logs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_logs(&path, out);
        } else if path.is_file() && path.extension().map(|e| e == "log").unwrap_or(false) {
            out.push(path);
        }
    }
}

pub fn scan(roots: &Roots, key: &str, label: &str, errors: &mut Vec<String>) -> Vec<RawRow> {
    let root = roots.trae_plugin();
    let sessions = collect_sessions(&root, errors);
    if sessions.is_empty() {
        let db = root
            .join("ai-chat")
            .join("AppData")
            .join("IDEA")
            .join("ai-agent")
            .join("database.db");
        if db.exists() {
            errors.push(format!(
                "{label}：主会话库为 SQLCipher 加密，当前从插件运行日志解析；若日志为空，请在 Trae 插件中重新打开历史会话或导出"
            ));
        } else {
            errors.push(format!(
                "{label}：未在 ~/.marscode/logs 解析到 get_messages 响应"
            ));
        }
    }

    sessions
        .into_iter()
        .map(|(session_id, session)| {
            let title = session
                .messages
                .values()
                .find(|m| m.role == "user")
                .map(|m| m.text.clone())
                .unwrap_or_else(|| format!("Trae 插件会话 {}", short_id(&session_id)));
            let mut row = RawRow::empty("trae_plugin", key, label);
            row.session_id = session_id;
            row.title = title.chars().take(100).collect();
            row.last_at = session.last_at;
            row.msgs = session.messages.len() as i64;
            row.size = session.size;
            row.state = SessionState::Visible;
            row.has_body = !session.messages.is_empty();
            row.origin = "ide-plugin".into();
            row
        })
        .collect()
}

pub fn preview(roots: &Roots, _key: &str, session_id: &str) -> Option<Vec<(String, String, i64)>> {
    let root = roots.trae_plugin();
    let mut errors = Vec::new();
    let sessions = collect_sessions(&root, &mut errors);
    let session = sessions.get(session_id)?;
    let mut items: Vec<&Message> = session.messages.values().collect();
    items.sort_by_key(|m| (m.index, m.time));
    Some(
        items
            .into_iter()
            .map(|m| (m.role.clone(), m.text.clone(), m.time))
            .collect(),
    )
}

fn collect_sessions(root: &Path, errors: &mut Vec<String>) -> HashMap<String, Session> {
    let mut sessions: HashMap<String, Session> = HashMap::new();
    for file in log_files(root) {
        if let Err(e) = scan_log(&file, &mut sessions) {
            errors.push(format!(
                "Trae CN 插件：{} 读取失败（{e}）",
                file.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }
    sessions
}

fn scan_log(path: &Path, sessions: &mut HashMap<String, Session>) -> Result<(), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let Some(value) = parse_response(&line) else {
                    continue;
                };
                let Some(items) = value
                    .get("data")
                    .and_then(|data| data.get("items"))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                for item in items {
                    ingest_item(item, sessions);
                }
            }
        }
    }
    Ok(())
}

fn parse_response(line: &str) -> Option<Value> {
    let marker = "lite/get_messages response: ";
    let idx = line.find(marker)?;
    let json = line[idx + marker.len()..].trim();
    if !json.starts_with('{') {
        return None;
    }
    serde_json::from_str::<Value>(json).ok()
}

fn ingest_item(item: &Value, sessions: &mut HashMap<String, Session>) {
    let Some(session_id) = item.get("chat_session_id").and_then(Value::as_str) else {
        return;
    };
    let Some(message_id) = item.get("message_id").and_then(Value::as_str) else {
        return;
    };
    let role = item.get("role").and_then(Value::as_str).unwrap_or("");
    if role != "user" && role != "assistant" {
        return;
    }
    let index = item
        .get("message_index")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let time = normalize_ms(item.get("created_at").and_then(Value::as_i64).unwrap_or(0));
    let content = item.get("content").and_then(Value::as_str).unwrap_or("");
    let text = extract_text(item, content);
    if text.is_empty() {
        return;
    }
    let size = content.len() as i64;
    let session = sessions.entry(session_id.to_string()).or_default();
    session.last_at = session.last_at.max(time);
    session.size += size;
    session.messages.insert(
        message_id.to_string(),
        Message {
            index,
            role: role.to_string(),
            text,
            time,
        },
    );
}

fn extract_text(item: &Value, raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let parsed = serde_json::from_str::<Value>(raw).ok();
    match parsed.as_ref().or(Some(item)) {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.get("text_content")
                    .or_else(|| part.get("text"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(obj)) => {
            if let Some(messages) = obj.get("messages").and_then(Value::as_array) {
                for message in messages {
                    let plan = message.get("plan_item").unwrap_or(message);
                    let tool = plan.get("tool_call_info");
                    if let Some(name) = tool.and_then(|v| v.get("name")).and_then(Value::as_str) {
                        let params = tool.and_then(|v| v.get("params"));
                        let value = if name == "finish" {
                            params.and_then(|p| p.get("summary"))
                        } else if name == "ReplyUser" {
                            params.and_then(|p| p.get("message")).or_else(|| {
                                params
                                    .and_then(|p| p.get("params"))
                                    .and_then(|q| q.get("message"))
                            })
                        } else {
                            None
                        };
                        if let Some(text) = value.and_then(Value::as_str) {
                            if !text.is_empty() {
                                return text.to_string();
                            }
                        }
                    }
                    if let Some(text) = plan.get("reasoning_content").and_then(Value::as_str) {
                        if !text.trim().is_empty() {
                            return text.trim().to_string();
                        }
                    }
                }
            }
            if let Some(text) = obj.get("text_content").and_then(Value::as_str) {
                return text.to_string();
            }
            raw.to_string()
        }
        _ => raw.to_string(),
    }
}

fn normalize_ms(value: i64) -> i64 {
    if value > 0 && value < 1_000_000_000_000 {
        value * 1000
    } else {
        value
    }
}

fn short_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn mtime_ms(stat: &std::fs::Metadata) -> u128 {
    stat.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_get_messages_log() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let log = root
            .join(".marscode")
            .join("logs")
            .join("connections")
            .join("a.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        let response = serde_json::json!({
            "code": 0,
            "data": {
                "items": [
                    {
                        "chat_session_id": "s1",
                        "message_id": "m1",
                        "message_index": 1,
                        "role": "user",
                        "created_at": 1_794_278_584,
                        "content": "[{\"type\":\"text\",\"text_content\":\"你好，你是什么模型\"}]"
                    },
                    {
                        "chat_session_id": "s1",
                        "message_id": "m2",
                        "message_index": 2,
                        "role": "assistant",
                        "created_at": 1_794_278_589,
                        "content": "{\"messages\":[{\"plan_item\":{\"tool_call_info\":{\"name\":\"finish\",\"params\":{\"summary\":\"我是 Seed-Code\"}}}}]}"
                    }
                ]
            }
        });
        std::fs::write(
            &log,
            format!(
                "2026-11-10 11:06:46 [info] [LiteRouteDebug] trae/invoke lite/get_messages response: {}\n",
                response
            ),
        )
        .unwrap();

        let roots = Roots::at(root, root, root);
        let mut errors = Vec::new();
        let rows = scan(&roots, "trae_plugin_cn", "Trae CN 插件", &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "你好，你是什么模型");
        assert_eq!(rows[0].msgs, 2);
        assert_eq!(rows[0].origin, "ide-plugin");

        let messages = preview(&roots, "trae_plugin_cn", "s1").unwrap();
        assert_eq!(messages[0].0, "user");
        assert_eq!(messages[0].1, "你好，你是什么模型");
        assert_eq!(messages[1].0, "assistant");
        assert_eq!(messages[1].1, "我是 Seed-Code");
    }
}
