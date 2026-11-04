//! 会话正文预览：把三种落盘格式读成人能看的对话。
//!
//! 三种来源（`docs/phase-4-sessions.md` §3）：
//!   - WorkBuddy：`projects/<slug>/<sid>.jsonl`，一行一个事件，只认 `type=message`
//!   - CodeBuddy：`<history>/<工作区>/<id>/messages/*.json`，一条消息一个文件
//!   - Qoder / ZCode：SQLite 里的消息行
//!
//! **只取头尾**：一条会话可能几十 MB，全读进内存会把界面拖住。按「消息条数」截断
//! 而不是按字节——单条工具结果动辄几十 KB，按字节切经常只切到两三条。

use crate::app::clients::Roots;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::sessions::sources::wb;
use rusqlite::Connection;
use serde::Serialize;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

// 前端（ui 的 PreviewBody）已做渐进渲染与长文本折叠，这里可以放宽到
// 「预览可用 100 条」：单条截 3000 字，全量内存上限 ~1MB，界面仍只挂 50 条/批。
const HEAD_MESSAGES: usize = 40;
const TAIL_MESSAGES: usize = 60;
const TAIL_BYTES: u64 = 1024 * 1024;
const CB_SCAN_LIMIT: usize = 600;
const MAX_MESSAGES: usize = 100;
const MAX_CHARS: usize = 3000;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: String,
    pub text: String,
    /// 毫秒时间戳，0 表示这条没带时间
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub time: i64,
    /// 只用于头尾合并去重，不给界面
    #[serde(skip)]
    pub id: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Default)]
pub struct Preview {
    pub messages: Vec<Message>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub dropped: usize,
    pub truncated: bool,
    pub note: String,
}

/// 定位一条会话的正文需要的最小信息。由行 key 解析而来。
#[derive(Debug, Clone)]
pub struct Target {
    pub kind: String,
    pub source_key: String,
    pub account: String,
    pub session_id: String,
}

/// 行 key 的格式是 `kind|source_key|account|session_id`（与界面 `data-key` 一致）。
pub fn parse_key(key: &str) -> Result<Target> {
    let parts: Vec<&str> = key.splitn(4, '|').collect();
    if parts.len() != 4 {
        return Err(AppError::new(ErrorKind::Invalid, "会话标识格式不对"));
    }
    Ok(Target {
        kind: parts[0].to_string(),
        source_key: parts[1].to_string(),
        account: parts[2].to_string(),
        session_id: parts[3].to_string(),
    })
}

pub fn load(roots: &Roots, target: &Target) -> Preview {
    if target.kind == "trae" {
        return Preview {
            note: "这条对话存在云端，本地只有输入历史，没有可读正文".into(),
            ..Default::default()
        };
    }
    match super::backend::backend_for(&target.kind) {
        Some(backend) => backend.load_preview(roots, target),
        None => Preview {
            note: format!("不认识的会话类型：{}", target.kind),
            ..Default::default()
        },
    }
}

// ------------------------------------------------------------------ 公共小工具

/// 工具注入的上下文块会整段塞进首条消息，展示时要去掉。
const BLOCKS: [&str; 4] = [
    "system-reminder",
    "user_info",
    "project_context",
    "additional_data",
];

fn clean(text: &str) -> String {
    let mut out = text.to_string();
    for tag in BLOCKS {
        out = strip_tag(&out, tag);
    }
    out = out.replace("<user_query>", "").replace("</user_query>", "");
    out.trim().to_string()
}

/// 去掉 `<tag ...>…</tag>`，大小写不敏感。手写而不是引 regex：
/// 省 1MB 体积，规则也就这么四条。
fn strip_tag(text: &str, tag: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(start) = lower[cursor..].find(&open).map(|i| cursor + i) {
        match lower[start..].find(&close) {
            Some(offset) => {
                out.push_str(&text[cursor..start]);
                cursor = start + offset + close.len();
            }
            None => break,
        }
    }
    out.push_str(&text[cursor.min(text.len())..]);
    out
}

impl Message {
    fn dedup_key(&self) -> String {
        if !self.id.is_empty() {
            return self.id.clone();
        }
        format!("{}|{}", self.role, self.text)
    }
}

fn clip(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_CHARS {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(MAX_CHARS).collect();
    cut = cut.trim_end().to_string();
    cut.push_str(" …");
    cut
}

/// content 可能是字符串，也可能是 `[{type:"text", text:…}]` 片段列表。
fn join_content(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Object(map) => {
                    let kind = map.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if kind == "text" || kind == "input_text" || kind == "output_text" {
                        map.get("text").and_then(|v| v.as_str()).map(str::to_string)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// 太长时保留开头和结尾，中间用一条占位说明。
fn thin(messages: Vec<Message>) -> (Vec<Message>, usize) {
    if messages.len() <= MAX_MESSAGES {
        return (messages, 0);
    }
    let head = (MAX_MESSAGES / 3).max(2);
    let tail = MAX_MESSAGES - head;
    let dropped = messages.len() - head - tail;
    let mut out: Vec<Message> = messages[..head].to_vec();
    out.push(Message {
        role: "gap".into(),
        text: format!("中间省略 {dropped} 条"),
        time: 0,
        id: String::new(),
    });
    out.extend_from_slice(&messages[messages.len() - tail..]);
    (out, dropped)
}

fn result(messages: Vec<Message>, truncated: bool, note: &str) -> Preview {
    let total = messages.len();
    let (shown, dropped) = thin(messages);
    let note = if !note.is_empty() {
        note.to_string()
    } else if truncated {
        "文件较大，只读取了开头与结尾".to_string()
    } else if dropped > 0 {
        format!("中间省略 {dropped} 条")
    } else {
        String::new()
    };
    Preview {
        messages: shown,
        total,
        dropped,
        truncated,
        note,
    }
}

// ------------------------------------------------------------------ WorkBuddy

pub(crate) fn load_workbuddy(roots: &Roots, target: &Target) -> Preview {
    let Some(ep) = wb::editions(roots)
        .into_iter()
        .find(|ep| ep.key == target.source_key)
    else {
        return result(Vec::new(), false, "客户端目录不存在");
    };
    let files = wb::project_files(&ep.projects);
    let owned = files.get(&target.session_id).cloned().unwrap_or_default();
    let Some(path) = owned
        .iter()
        .find(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))
    else {
        return result(Vec::new(), false, "没有正文文件");
    };

    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size <= TAIL_BYTES {
        let mut messages = Vec::new();
        if let Ok(file) = File::open(path) {
            for line in BufReader::new(file)
                .split(b'\n')
                .map_while(std::result::Result::ok)
            {
                if let Some(message) = parse_wb_line(&line) {
                    messages.push(message);
                }
            }
        }
        return result(messages, false, "");
    }

    let head = read_head_messages(path, HEAD_MESSAGES);
    let mut tail = read_tail_messages(path, TAIL_MESSAGES);
    // 头尾可能重叠（文件不大的时候），按消息 id 去重；没有 id 的退化成「角色+正文」
    let seen: Vec<String> = head.iter().map(Message::dedup_key).collect();
    tail.retain(|m| !seen.contains(&m.dedup_key()));
    let mut merged = head;
    merged.extend(tail);
    result(merged, true, "")
}

fn parse_wb_line(line: &[u8]) -> Option<Message> {
    if !line.windows(9).any(|w| w == b"\"message\"") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    if value.get("type").and_then(|v| v.as_str()) != Some("message") {
        return None;
    }
    let role = value.get("role").and_then(|v| v.as_str())?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let text = clean(&join_content(value.get("content")?));
    if text.is_empty() {
        return None;
    }
    Some(Message {
        role: role.to_string(),
        text: clip(&text),
        time: value.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0),
        id: value
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn read_head_messages(path: &Path, limit: usize) -> Vec<Message> {
    let mut out = Vec::new();
    let Ok(file) = File::open(path) else {
        return out;
    };
    for line in BufReader::new(file)
        .split(b'\n')
        .map_while(std::result::Result::ok)
    {
        if let Some(message) = parse_wb_line(&line) {
            out.push(message);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn read_tail_messages(path: &Path, limit: usize) -> Vec<Message> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    if size > TAIL_BYTES && file.seek(SeekFrom::End(-(TAIL_BYTES as i64))).is_err() {
        return Vec::new();
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return Vec::new();
    }
    // 第一行多半是从中间截断的，丢掉
    let start = buffer
        .iter()
        .position(|b| *b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut out: Vec<Message> = buffer[start..]
        .split(|b| *b == b'\n')
        .filter_map(parse_wb_line)
        .collect();
    if out.len() > limit {
        out = out.split_off(out.len() - limit);
    }
    out
}

// ------------------------------------------------------------------ CodeBuddy

pub(crate) fn load_codebuddy(roots: &Roots, target: &Target) -> Preview {
    // 会话目录在「工作区/<会话 id>/messages」，工作区名不固定：
    // 逐个工作区试一下比在索引里存一份 slug 更经得起客户端改目录结构
    let history = roots
        .codebuddy()
        .join(&target.account)
        .join(&target.source_key)
        .join(&target.account)
        .join("history");
    let mut dir = None;
    if let Ok(entries) = std::fs::read_dir(&history) {
        for entry in entries.flatten() {
            let candidate = entry.path().join(&target.session_id).join("messages");
            if candidate.is_dir() {
                dir = Some(candidate);
                break;
            }
        }
    }
    let Some(dir) = dir else {
        return result(Vec::new(), false, "没有正文目录（工作区可能已被删除）");
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return result(Vec::new(), false, "正文目录读不出来");
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());

    // 从**最后**往前读：CodeBuddy 一条会话几百个文件，且大量是 tool 调用。
    // 开头那段多半是「读文件」的噪音，用户想看的是最近的对话。
    let total_files = files.len();
    let mut messages: Vec<Message> = Vec::new();
    let mut scanned = 0usize;
    for file in files.iter().rev() {
        scanned += 1;
        if scanned > CB_SCAN_LIMIT {
            break;
        }
        if let Some(message) = parse_codebuddy(file) {
            messages.push(message);
            if messages.len() >= MAX_MESSAGES + HEAD_MESSAGES {
                break;
            }
        }
    }
    messages.reverse();
    result(messages, scanned < total_files, "")
}

fn parse_codebuddy(file: &Path) -> Option<Message> {
    let text = std::fs::read_to_string(file).ok()?;
    let record: serde_json::Value = serde_json::from_str(&text).ok()?;
    let role = record.get("role").and_then(|v| v.as_str())?;
    if role != "user" && role != "assistant" {
        return None;
    }
    // 有的版本把 message 再序列化成字符串
    let body = match record.get("message") {
        Some(serde_json::Value::String(raw)) => serde_json::from_str::<serde_json::Value>(raw)
            .unwrap_or(serde_json::Value::String(raw.clone())),
        Some(other) => other.clone(),
        None => serde_json::Value::Null,
    };
    let content = body.get("content").cloned().unwrap_or(body);
    let text = clean(&join_content(&content));
    if text.is_empty() {
        return None;
    }
    Some(Message {
        role: role.to_string(),
        text: clip(&text),
        time: 0,
        id: String::new(),
    })
}

// ------------------------------------------------------------------ Qoder

pub(crate) fn load_qoder(roots: &Roots, target: &Target) -> Preview {
    let intl = target.source_key == "qoder_intl";
    let path = roots.qoder(intl).join("main.sqlite");
    let Some(conn) = open_ro(&path) else {
        return result(Vec::new(), false, "会话库无法读取（客户端可能正在写入）");
    };
    let mut messages = Vec::new();
    let stmt = conn.prepare(
        "SELECT message_id, payload_json FROM chat_session_messages
         WHERE session_id = ?1 ORDER BY sequence, created_at",
    );
    if let Ok(mut stmt) = stmt {
        if let Ok(rows) = stmt.query_map([&target.session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            for (mid, raw) in rows.flatten() {
                if let Some(message) = parse_qoder(&mid, &raw) {
                    messages.push(message);
                }
            }
        }
    }
    result(messages, false, "")
}

fn parse_qoder(mid: &str, raw: &str) -> Option<Message> {
    let data: serde_json::Value = serde_json::from_str(raw).ok()?;
    let role = data
        .get("role")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if mid.starts_with("assistant:") {
                "assistant".to_string()
            } else {
                "user".to_string()
            }
        });
    if role != "user" && role != "assistant" {
        return None;
    }
    let mut body = data
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if body.trim().is_empty() {
        // 新库把正文放在 parts[type=text]
        if let Some(parts) = data.get("parts").and_then(|v| v.as_array()) {
            body = parts
                .iter()
                .filter_map(|part| {
                    let kind = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if kind == "text" {
                        part.get("text").and_then(|v| v.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    let text = clean(&body);
    if text.is_empty() {
        return None;
    }
    Some(Message {
        role,
        text: clip(&text),
        time: 0,
        id: mid.to_string(),
    })
}

// ------------------------------------------------------------------ ZCode

pub(crate) fn load_zcode(roots: &Roots, target: &Target) -> Preview {
    let path = roots.zcode().join("cli").join("db").join("db.sqlite");
    let Some(conn) = open_ro(&path) else {
        return result(Vec::new(), false, "会话库无法读取（客户端可能正在写入）");
    };
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message WHERE session_id = ?1",
            [&target.session_id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let mut rows: Vec<(String, String)> = Vec::new();
    if (total as usize) > HEAD_MESSAGES + TAIL_MESSAGES {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT id, data FROM message WHERE session_id = ?1
             ORDER BY sequence, time_created LIMIT ?2",
        ) {
            if let Ok(items) = stmt.query_map(
                rusqlite::params![target.session_id, HEAD_MESSAGES as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            ) {
                rows.extend(items.flatten());
            }
        }
        if let Ok(mut stmt) = conn.prepare(
            "SELECT id, data FROM message WHERE session_id = ?1
             ORDER BY sequence DESC, time_created DESC LIMIT ?2",
        ) {
            if let Ok(items) = stmt.query_map(
                rusqlite::params![target.session_id, TAIL_MESSAGES as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            ) {
                let mut tail: Vec<(String, String)> = items.flatten().collect();
                tail.reverse();
                rows.extend(tail);
            }
        }
    } else if let Ok(mut stmt) = conn.prepare(
        "SELECT id, data FROM message WHERE session_id = ?1 ORDER BY sequence, time_created",
    ) {
        if let Ok(items) = stmt.query_map([&target.session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            rows.extend(items.flatten());
        }
    }

    // 正文在 part 表，一次查回来再归组
    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let mut texts: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    if !ids.is_empty() {
        let marks = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT message_id, data FROM part WHERE message_id IN ({marks}) ORDER BY sequence"
        );
        if let Ok(mut stmt) = conn.prepare(&sql) {
            let params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            if let Ok(items) = stmt.query_map(params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for (mid, raw) in items.flatten() {
                    let Ok(part) = serde_json::from_str::<serde_json::Value>(&raw) else {
                        continue;
                    };
                    if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                            texts.entry(mid).or_default().push(text.to_string());
                        }
                    }
                }
            }
        }
    }

    let mut messages = Vec::new();
    for (id, raw) in &rows {
        let Ok(meta) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        let role = meta.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        let text = clean(&texts.get(id).cloned().unwrap_or_default().join("\n"));
        if text.is_empty() {
            continue;
        }
        messages.push(Message {
            role: role.to_string(),
            text: clip(&text),
            time: 0,
            id: String::new(),
        });
    }
    // 与头尾截取同口径：只有「总量超出头尾之和」才是真的只读了片段；
    // 用 messages.len() 比，会把 system/空正文行误算成被截断
    let truncated = total as usize > HEAD_MESSAGES + TAIL_MESSAGES;
    result(messages, truncated, "")
}

fn open_ro(path: &Path) -> Option<Connection> {
    if !path.exists() {
        return None;
    }
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_injected_blocks() {
        let text = "<system-reminder>别显示我</system-reminder>你好<user_info>x</user_info>";
        assert_eq!(clean(text), "你好");
    }

    #[test]
    fn joins_content_parts() {
        let value: serde_json::Value = serde_json::from_str(
            r#"[{"type":"text","text":"第一段"},{"type":"image"},{"type":"text","text":"第二段"}]"#,
        )
        .unwrap();
        assert_eq!(join_content(&value), "第一段\n第二段");
        assert_eq!(
            join_content(&serde_json::Value::String("纯文本".into())),
            "纯文本"
        );
    }

    #[test]
    fn thins_long_conversations() {
        let messages: Vec<Message> = (0..200)
            .map(|i| Message {
                role: "user".into(),
                text: format!("m{i}"),
                time: i as i64,
                id: String::new(),
            })
            .collect();
        let (shown, dropped) = thin(messages);
        assert_eq!(
            shown.len(),
            MAX_MESSAGES + 1,
            "头 + 占位 1 + 尾 = MAX_MESSAGES + 1"
        );
        let head = (MAX_MESSAGES / 3).max(2);
        assert_eq!(dropped, 200 - MAX_MESSAGES);
        assert_eq!(shown[head].role, "gap");
    }

    #[test]
    fn clips_long_text() {
        let long = "字".repeat(MAX_CHARS + 50);
        let cut = clip(&long);
        assert!(cut.ends_with('…'));
        assert_eq!(cut.chars().count(), MAX_CHARS + 2);
    }

    #[test]
    fn parses_key() {
        let target = parse_key("workbuddy|cn|uid-1|abc|def").unwrap();
        assert_eq!(target.kind, "workbuddy");
        assert_eq!(target.session_id, "abc|def", "会话 id 里有竖线也不能截断");
        assert!(parse_key("bad").is_err());
    }
}
