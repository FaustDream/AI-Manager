//! Qoder / Qoder CN 的 JetBrains 插件会话（实测 2026-11）。
//!
//! 正源是 `~/.qoder{,-cn}/cache/projects/<项目>/conversation-history/<会话id>/<会话id>.jsonl`，
//! 每行顶层是 `role`（user/assistant，与独立版 Qoder 的 `type` 字段不同名）+
//! `message.content[].text`；没有逐行时间戳，会话时间取文件 mtime（插件边聊边刷盘，
//! mtime 就是最后活跃）。标题从 `<user_query>` 段提取，框架注入的
//! `<system-reminder>` / `<ide_context>` 等块不进标题。旧的 `~/.qoder*/projects/**.jsonl`
//! 布局实测只剩子 agent 缓存（内容全是 system-reminder），不是用户会话，不再扫描。
//! 只读；删除走本地隐藏（插件没有可重放的删除接口）。

use crate::app::clients::Roots;
use crate::app::sessions::sources::SourceInfo;
use crate::app::sessions::store::{RawRow, UNNAMED_TITLE};
use crate::types::SessionState;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const EDITIONS: [(&str, &str, bool); 2] = [
    ("qoder_plugin_cn", "Qoder CN 插件", true),
    ("qoder_plugin_intl", "Qoder 插件", false),
];

pub fn sources(roots: &Roots) -> Vec<SourceInfo> {
    EDITIONS
        .iter()
        .map(|(key, label, cn)| {
            let root = roots.qoder_plugin(*cn);
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
    for file in session_files(root) {
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
        // v3：正源从 projects 换到 cache/conversation-history、字段从 type 改为 role，强制存量重扫。
        format!("v3|{}", parts.join("|"))
    }
}

/// 会话文件：`<root>/cache/projects/<项目>/conversation-history/<sid>/<sid>.jsonl`。
/// 只枚举到 conversation-history 一层——`~/.qoder*` 下 memory/skills/tmp 全在里面，整棵递归太宽。
fn session_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for project in sub_dirs(&root.join("cache").join("projects")) {
        for session in sub_dirs(&project.join("conversation-history")) {
            let name = session.file_name().unwrap_or_default().to_string_lossy();
            let file = session.join(format!("{name}.jsonl"));
            if file.is_file() {
                out.push(file);
            }
        }
    }
    out.sort();
    out
}

/// 目录下的子目录列表（缺目录就是没装，静默为空）。
fn sub_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if entry.path().is_dir() {
                out.push(entry.path());
            }
        }
    }
    out
}

pub fn scan(roots: &Roots, key: &str, label: &str, errors: &mut Vec<String>) -> Vec<RawRow> {
    let root = roots.qoder_plugin(key == "qoder_plugin_cn");
    let mut rows = Vec::new();
    for file in session_files(&root) {
        match scan_file(&file, key, label) {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => {}
            Err(e) => errors.push(format!(
                "{label}：{} 解析失败（{e}）",
                file.file_name().unwrap_or_default().to_string_lossy()
            )),
        }
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.last_at));
    rows
}

fn scan_file(path: &Path, key: &str, label: &str) -> Result<Option<RawRow>, String> {
    let session_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| "缺少会话 id".to_string())?;
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut title = String::new();
    let mut last_at = 0i64;
    let mut msgs = 0i64;
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let kind = message_role(&value);
                if kind.is_empty() {
                    continue;
                }
                let text = extract_message_text(&value);
                if text.is_empty() {
                    continue;
                }
                msgs += 1;
                if kind == "user" && title.is_empty() {
                    title = clean_title_text(&text);
                }
                // 个别导出行可能带 timestamp，有就用、没有退回 mtime（在行尾统一处理）。
                if let Some(ts) = parse_iso_ms(value.get("timestamp").and_then(Value::as_str)) {
                    last_at = last_at.max(ts);
                }
            }
        }
    }

    if msgs == 0 {
        return Ok(None);
    }
    if last_at == 0 {
        last_at = file_mtime_ms(path);
    }
    let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    let mut row = RawRow::empty("qoder_plugin", key, label);
    row.session_id = session_id;
    row.title = if title.is_empty() {
        UNNAMED_TITLE.into()
    } else {
        title
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or(UNNAMED_TITLE)
            .chars()
            .take(100)
            .collect()
    };
    row.last_at = last_at;
    row.msgs = msgs;
    row.size = size;
    row.state = SessionState::Visible;
    row.has_body = true;
    row.origin = "ide-plugin".into();
    Ok(Some(row))
}

/// 顶层 role 优先（conversation-history 实测格式），兼容 `type`（旧 projects 布局）。
fn message_role(value: &Value) -> &'static str {
    let field = ["role", "type"];
    for name in field {
        match value.get(name).and_then(Value::as_str) {
            Some("user") => return "user",
            Some("assistant") => return "assistant",
            _ => {}
        }
    }
    ""
}

/// 标题用的净化：优先取 `<user_query>` 段（框架包装把真实提问包在里面）。
/// 段内开头挂着的 `#file:` 附件引用是用户消息自带的，不算提问本体，剥掉；
/// 实测有的行只有开标签没有闭标签（后面还跟着 reminder 块或附件行），
/// 这时从开标签往后取第一条不像标记的文本行；都没有则剥掉成对标签块后取原文。
fn clean_title_text(text: &str) -> String {
    if let Some(query) = extract_block(text, "user_query") {
        let mut query = query.trim();
        // 段首挂着用户 @ 进来的文件引用，不算提问本体：`#file:路径`（IDE 记法）剥掉空格/换行前的路径体；
        // `'x:\\路径'`（用户打字引用，引号内含盘符才剥）剥掉整个引号段。
        if let Some(rest) = query.strip_prefix("#file:") {
            let head = rest.split([' ', '\n']).next().unwrap_or_default();
            query = rest[head.len()..].trim_start_matches([' ', '\n']);
        } else if query.starts_with('\'') || query.starts_with('"') {
            let quote = &query[..1];
            if let Some(end) = query[1..].find(quote) {
                let quoted = &query[1..1 + end];
                if quoted.contains(":\\") || quoted.contains(":/") {
                    query = query[1 + end + 1..].trim_start_matches([' ', '\n']);
                }
            }
        }
        // 标题只留第一条文本行（列表列高本来就一行，多行提问取首行）。
        return query
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default()
            .to_string();
    }
    if let Some(start) = text.find("<user_query>") {
        for line in text[start + "<user_query>".len()..].lines() {
            let line = line.trim();
            if !line.is_empty()
                && !line.starts_with('#')
                && !line.starts_with('<')
                && !line.contains("</")
            {
                return line.to_string();
            }
        }
        return String::new();
    }
    strip_markup(text)
}

/// 提取 `<tag>…</tag>` 内容；只找到开标签没有闭标签时返回 None（不误剥）。
fn extract_block(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)?;
    Some(text[start..start + end].to_string())
}

/// 去掉成对的框架标签块（system-reminder / ide_context 等），保留其余原文。
fn strip_markup(text: &str) -> String {
    const TAGS: [&str; 6] = [
        "system-reminder",
        "system_reminder",
        "attached_files",
        "ide_context",
        "user_info",
        "agents_instructions",
    ];
    let mut out = text.to_string();
    for tag in TAGS {
        while let Some(block) = extract_block(&out, tag) {
            let rebuilt = out.replace(&format!("<{tag}>{block}</{tag}>"), "");
            if rebuilt == out {
                break;
            }
            out = rebuilt;
        }
    }
    out.trim().to_string()
}

pub fn preview(roots: &Roots, key: &str, session_id: &str) -> Option<Vec<(String, String, i64)>> {
    let root = roots.qoder_plugin(key == "qoder_plugin_cn");
    let file = session_files(&root).into_iter().find(|p| {
        p.file_stem()
            .map(|s| s.to_string_lossy() == session_id)
            .unwrap_or(false)
    })?;
    let stamp = file_mtime_ms(&file);
    let text = std::fs::read_to_string(file).ok()?;
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = message_role(&value);
        if role.is_empty() {
            continue;
        }
        let body = extract_message_text(&value);
        if body.is_empty() {
            continue;
        }
        let time = parse_iso_ms(value.get("timestamp").and_then(Value::as_str)).unwrap_or(stamp);
        out.push((role.into(), body, time));
    }
    Some(out)
}

fn extract_message_text(value: &Value) -> String {
    let message = match value.get("message") {
        Some(Value::String(text)) => return text.clone(),
        Some(m) => m,
        None => {
            return value
                .get("humanInput")
                .and_then(|v| v.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        }
    };
    let content = message.get("content").unwrap_or(message);
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.as_str() {
                    return Some(text.to_string());
                }
                let obj = item.as_object()?;
                let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
                if kind == "text" || kind == "input_text" || kind == "output_text" {
                    obj.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn parse_iso_ms(value: Option<&str>) -> Option<i64> {
    let text = value?;
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(dt.timestamp_millis());
    }
    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .and_then(|dt| dt.and_local_timezone(chrono::Local).single())
        .map(|dt| dt.timestamp_millis())
}

fn file_mtime_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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

    /// 建一个正源布局：<root>/cache/projects/<项目>/conversation-history/<sid>/<sid>.jsonl
    fn write_session(root: &Path, project: &str, sid: &str, content: &str) -> PathBuf {
        let session = root
            .join("cache")
            .join("projects")
            .join(project)
            .join("conversation-history")
            .join(sid);
        std::fs::create_dir_all(&session).unwrap();
        let file = session.join(format!("{sid}.jsonl"));
        std::fs::write(&file, content).unwrap();
        file
    }

    #[test]
    fn scans_conversation_history_and_preview() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        // 实测格式：顶层 role，用户首行是框架包装，<user_query> 里才是真实提问。
        write_session(
            &root.join(".qoder"),
            "AI Manager-abc",
            "s1",
            &[
                serde_json::json!({
                    "role": "user",
                    "message": {"content": [{"type": "text", "text": "<attached_files>#file:C:\\x\\AGENTS.md</attached_files>\n<system-reminder>ctx</system-reminder>\n<user_query>请为当前项目把会话聚合</user_query>"}]}
                }),
                serde_json::json!({
                    "role": "assistant",
                    "message": {"content": [{"type": "thinking", "thinking": "ignore"}, {"type": "text", "text": "我是 Qoder"}]}
                }),
            ]
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        );
        // 没有逐行 timestamp：时间应回退到文件 mtime（刚写完 ≈ 当前时间）。
        let roots = Roots::at(root, root, root);
        let mut errors = Vec::new();
        let rows = scan(&roots, "qoder_plugin_intl", "Qoder 插件", &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "请为当前项目把会话聚合");
        assert_eq!(rows[0].msgs, 2);
        assert_eq!(rows[0].origin, "ide-plugin");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!((now - rows[0].last_at).abs() < 86_400_000, "mtime 兜底失效");

        let messages = preview(&roots, "qoder_plugin_intl", "s1").unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages[0].1.contains("请为当前项目把会话聚合"));
        assert_eq!(messages[1].1, "我是 Qoder");
    }

    #[test]
    fn cn_and_intl_stay_separate_and_ignore_stray_layouts() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let line = |text: &str| {
            serde_json::json!({"role": "user", "message": {"content": [{"type": "text", "text": text}]}})
                .to_string()
        };
        write_session(&root.join(".qoder"), "p", "intl1", &line("国际版会话"));
        write_session(&root.join(".qoder-cn"), "p", "cn1", &line("国内版会话"));
        // 旧 projects 布局里的子 agent 缓存不该再被扫到。
        let stray = root.join(".qoder").join("projects").join("p");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("old.jsonl"), line("不该出现")).unwrap();

        let roots = Roots::at(root, root, root);
        let intl = scan(&roots, "qoder_plugin_intl", "Qoder 插件", &mut Vec::new());
        let cn = scan(&roots, "qoder_plugin_cn", "Qoder CN 插件", &mut Vec::new());
        assert_eq!(intl.len(), 1, "{intl:?}");
        assert_eq!(intl[0].title, "国际版会话");
        assert_eq!(cn.len(), 1);
        assert_eq!(cn[0].title, "国内版会话");
        assert_eq!(cn[0].source_key, "qoder_plugin_cn");
    }

    #[test]
    fn title_survives_unclosed_query_tag_and_attached_files() {
        // 真实首行形态：引用了 AGENTS.md 的那条只有 `<user_query>` 开标签没有闭标签，
        // 前面还挂着附件行——标题只留第一条像提问的行。
        let text = "<attached_files>\n#file:C:\\x\\AGENTS.md\n</attached_files>\n<user_query>请为当前项目集成会话\n<system-reminder>ctx</system-reminder>";
        assert_eq!(clean_title_text(text), "请为当前项目集成会话");
        // 完全没闭合且后面没有杂质：取开标签后的内容。
        let open_only = "<attached_files>dump</attached_files><user_query>后半句还在流式里";
        assert_eq!(clean_title_text(open_only), "后半句还在流式里");
        // 没有包装块的普通提问原样保留。
        assert_eq!(clean_title_text("你好，你是什么模型"), "你好，你是什么模型");
        // 真实形态：`<user_query>` 闭合，但段首挂着用户自己 @ 进来的 #file 引用（空格后接正文）。
        let with_file = "<user_query>#file:D:\\AGENTS.md 请为当前项目集成会话\n第二段</user_query>";
        assert_eq!(clean_title_text(with_file), "请为当前项目集成会话");
        // 用户打字引用的 `'D:\\x.md' 正文`：引号内含盘符才剥，留提问本体。
        let typed = "<user_query>\n'D:\\AGENTS.md' 请为当前项目集成会话\n第二段</user_query>";
        assert_eq!(clean_title_text(typed), "请为当前项目集成会话");
        // 普通引号（不含盘符路径）不剥，防误伤真实提问。
        let quoted_words = "<user_query>「按软件分组」与 'sync' 都要</user_query>";
        assert_eq!(
            clean_title_text(quoted_words),
            "「按软件分组」与 'sync' 都要"
        );
        // #file 独占一行、换行后才是提问：取下一行。
        let file_only = "<user_query>\n#file:C:\\x\\a.md\n真正的提问\n</user_query>";
        assert_eq!(clean_title_text(file_only), "真正的提问");
        // 整段只有 #file 引用、没有正文：退回空串，由上层给「未命名」。
        assert_eq!(
            clean_title_text("<user_query>#file:C:\\x\\a.md</user_query>"),
            ""
        );
    }
}
