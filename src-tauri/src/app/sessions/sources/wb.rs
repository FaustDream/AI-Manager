//! WorkBuddy 国内版 / 国际版的会话。
//!
//! 落盘构成：`workbuddy.db` 的 `sessions` 行 + `projects/<slug>/<id>.jsonl` 正文
//! + 同名前缀的附属文件。标题与时间在库里，消息条数与体积得读文件——这是本阶段
//!   最慢的一段，所以源指纹里带上 `projects` 下每个条目的 mtime/size。

use crate::app::clients::Roots;
use crate::app::sessions::sources::MsgCache;
use crate::app::sessions::store::RawRow;
use crate::types::SessionState;

use rusqlite::Connection;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const LABELS: [(&str, &str); 2] = [("cn", "WorkBuddy 国内版"), ("intl", "WorkBuddy 国际版")];

pub struct Edition {
    pub key: &'static str,
    pub label: &'static str,
    pub root: PathBuf,
    pub projects: PathBuf,
}

impl Edition {
    pub fn db(&self) -> PathBuf {
        self.root.join("workbuddy.db")
    }
    pub fn snapshot(&self) -> PathBuf {
        self.root
            .join("storage")
            .join("skeleton")
            .join("account-snapshot.json")
    }
}

pub fn editions(roots: &Roots) -> Vec<Edition> {
    LABELS
        .iter()
        .filter_map(|(key, label)| {
            let root = roots.workbuddy(*key == "intl");
            if !root.join("workbuddy.db").exists() {
                return None;
            }
            Some(Edition {
                key,
                label,
                projects: root.join("projects"),
                root,
            })
        })
        .collect()
}

/// 当前登录的 uid：优先读 `account-snapshot.json`，读不到再从库里反推。
/// 归属判定（可见 / 旧账号）全靠它，读错了整份列表的状态就是错的。
pub fn current_uid(ep: &Edition) -> String {
    if let Ok(text) = std::fs::read_to_string(ep.snapshot()) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(uid) = json
                .get("primary")
                .and_then(|p| p.get("uid"))
                .and_then(|v| v.as_str())
            {
                if !uid.is_empty() {
                    return uid.to_string();
                }
            }
        }
    }
    let conn =
        match Connection::open_with_flags(ep.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(conn) => conn,
            Err(_) => return String::new(),
        };
    conn.query_row(
        "SELECT user_id FROM sessions WHERE transport='local'
         GROUP BY user_id ORDER BY COUNT(*) DESC LIMIT 1",
        [],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// 源指纹：库文件 + projects 下每个条目的元数据 + 当前 uid。
pub fn fingerprint(ep: &Edition) -> String {
    let mut parts = Vec::new();
    if let Ok(stat) = std::fs::metadata(ep.db()) {
        parts.push(format!("db:{}:{}", mtime_ms(&stat), stat.len()));
    }
    if let Ok(projects) = std::fs::read_dir(&ep.projects) {
        for project in projects.flatten() {
            let Ok(entries) = std::fs::read_dir(project.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(stat) = entry.metadata() else { continue };
                parts.push(format!(
                    "{}/{}:{}:{}",
                    project.file_name().to_string_lossy(),
                    entry.file_name().to_string_lossy(),
                    mtime_ms(&stat),
                    stat.len()
                ));
            }
        }
    }
    parts.sort();
    parts.push(format!("uid:{}", current_uid(ep)));
    parts.join("|")
}

fn mtime_ms(stat: &std::fs::Metadata) -> u128 {
    stat.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn scan(ep: &Edition, cache: &mut MsgCache, errors: &mut Vec<String>) -> Vec<RawRow> {
    let uid = current_uid(ep);
    let files = project_files(&ep.projects);
    let conn =
        match Connection::open_with_flags(ep.db(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(conn) => conn,
            Err(e) => {
                errors.push(format!("{}：会话库打不开（{e}）", ep.label));
                return Vec::new();
            }
        };
    let mut stmt = match conn.prepare("SELECT * FROM sessions") {
        Ok(stmt) => stmt,
        Err(e) => {
            errors.push(format!("{}：sessions 表读不出（{e}）", ep.label));
            return Vec::new();
        }
    };
    let iter = match stmt.query_map([], |row| {
        Ok(Row {
            id: text(row, "id").unwrap_or_default(),
            title: text(row, "custom_title").or_else(|| text(row, "title")),
            cwd: text(row, "cwd").unwrap_or_default(),
            user_id: text(row, "user_id").unwrap_or_default(),
            transport: text(row, "transport").unwrap_or_default(),
            deleted_at: number(row, "deleted_at"),
            last: number(row, "last_activity_at")
                .or_else(|| number(row, "created_at"))
                .unwrap_or(0),
        })
    }) {
        Ok(iter) => iter,
        Err(e) => {
            errors.push(format!("{}：sessions 行读不出（{e}）", ep.label));
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for item in iter.flatten() {
        if item.id.is_empty() {
            continue;
        }
        let owned = files.get(&item.id).cloned().unwrap_or_default();
        let (state, account) = match item.deleted_at {
            Some(value) if value != -1 => (SessionState::Deleted, uid.clone()),
            _ if !uid.is_empty() && !item.user_id.is_empty() && item.user_id != uid => {
                (SessionState::OldAccount, item.user_id.clone())
            }
            _ if item.transport == "cloud" => (SessionState::Cloud, uid.clone()),
            _ => (SessionState::Visible, uid.clone()),
        };
        // 按会话缓存：指纹（每个文件的 mtime:size）没变就直接复用，不重读正文
        let cache_key = format!("wb:{}:{}", ep.key, item.id);
        let sig = signature(&owned);
        let (msgs, size) = match cache.hit(&cache_key, &sig) {
            Some(hit) => (hit.msgs, hit.size),
            None => {
                let msgs = owned
                    .iter()
                    .filter(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false))
                    .map(|p| count_messages(p))
                    .sum();
                let size = owned.iter().map(|p| path_size(p)).sum();
                cache.put(&cache_key, &sig, msgs, size);
                (msgs, size)
            }
        };

        let mut row = RawRow::empty("workbuddy", ep.key, ep.label);
        row.account = account;
        row.session_id = item.id;
        row.title = item.title.unwrap_or_else(|| "（未命名会话）".into());
        row.cwd = item.cwd;
        row.last_at = item.last;
        row.msgs = msgs;
        row.size = size;
        row.state = state;
        row.has_body = !owned.is_empty();
        row.origin = "desktop".into();
        out.push(row);
    }
    out
}

/// 一个会话所有文件的元数据拼成的指纹：文件一改（含追加）就会变。
fn signature(files: &[PathBuf]) -> String {
    let mut parts: Vec<String> = files
        .iter()
        .map(|path| match std::fs::metadata(path) {
            Ok(stat) => format!(
                "{}:{}:{}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                mtime_ms(&stat),
                stat.len()
            ),
            Err(_) => "-".to_string(),
        })
        .collect();
    parts.sort();
    parts.join("|")
}

struct Row {
    id: String,
    title: Option<String>,
    cwd: String,
    user_id: String,
    transport: String,
    deleted_at: Option<i64>,
    last: i64,
}

fn text(row: &rusqlite::Row<'_>, name: &str) -> Option<String> {
    row.get::<_, Option<String>>(name).ok().flatten()
}

fn number(row: &rusqlite::Row<'_>, name: &str) -> Option<i64> {
    row.get::<_, Option<i64>>(name).ok().flatten()
}

/// `projects/<slug>/<id>.jsonl` 与同名前缀的附属文件，按会话 ID 归组。
/// 一次遍历目录，不逐条 glob——几百条会话时这个差别很明显。
pub fn project_files(projects: &Path) -> std::collections::HashMap<String, Vec<PathBuf>> {
    let mut out: std::collections::HashMap<String, Vec<PathBuf>> = std::collections::HashMap::new();
    let Ok(dirs) = std::fs::read_dir(projects) else {
        return out;
    };
    for dir in dirs.flatten() {
        let path = dir.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let stem = name.split('.').next().unwrap_or("").to_string();
            if !is_session_id(&stem) {
                continue;
            }
            out.entry(stem).or_default().push(entry.path());
        }
    }
    out
}

/// 正文之外的会话文件：`assistant-display` / `file-tree-manifests` 各有一份按 id
/// 命名的侧车，`logs/<日期>/sdk/conversations/` 下还有这个会话的日志。
///
/// 只搬 `projects/` 的话，删完客户端目录里还剩一堆按会话 id 命名的残渣，
/// 其中两份 json 本身就带着会话内容——「彻底删除」就名不副实了。
pub fn session_sidecars(ep: &Edition, id: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in ["assistant-display", "file-tree-manifests"] {
        let path = ep.root.join(dir).join(format!("{id}.json"));
        if path.is_file() {
            out.push(path);
        }
    }
    let Ok(days) = std::fs::read_dir(ep.root.join("logs")) else {
        return out;
    };
    for day in days.flatten() {
        let dir = day.path().join("sdk").join("conversations");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // 只认 `<id>.log` / `<id>-warning.log`：前缀相同但后面不带分隔符的
            // 不是这个会话的文件，别误伤
            if let Some(rest) = name.strip_prefix(id) {
                if rest.starts_with('.') || rest.starts_with('-') {
                    out.push(entry.path());
                }
            }
        }
    }
    out
}

/// 会话 ID 是 uuid / 32 位十六进制；projects 下别的名字不是正文。
fn is_session_id(value: &str) -> bool {
    value.len() >= 16
        && value
            .chars()
            .next()
            .map(|c| c.is_ascii_hexdigit())
            .unwrap_or(false)
        && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub fn path_size(path: &Path) -> i64 {
    let Ok(stat) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if stat.is_file() {
        return stat.len() as i64;
    }
    if !stat.is_dir() {
        return 0;
    }
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        total += path_size(&entry.path());
    }
    total
}

/// jsonl 里 `"type": "message"` 的行数。按行读，避免跨块把模式切断。
pub fn count_messages(path: &Path) -> i64 {
    let Ok(file) = File::open(path) else { return 0 };
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut line = Vec::new();
    let mut count = 0i64;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if is_message_line(&line) {
                    count += 1;
                }
            }
        }
    }
    count
}

fn is_message_line(line: &[u8]) -> bool {
    let needle = b"\"type\"";
    let mut from = 0;
    while let Some(pos) = find(&line[from..], needle) {
        let mut i = from + pos + needle.len();
        while i < line.len() && line[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < line.len() && line[i] == b':' {
            i += 1;
            while i < line.len() && line[i].is_ascii_whitespace() {
                i += 1;
            }
            if line[i..].starts_with(b"\"message\"") {
                return true;
            }
        }
        from += pos + needle.len();
    }
    false
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_shape() {
        assert!(is_session_id("0f3a9c2d8b7e4a1f9c3d5e7b9a1c2d3e"));
        assert!(is_session_id("0f3a9c2d-8b7e-4a1f-9c3d-5e7b9a1c2d3e"));
        assert!(!is_session_id("index.json"));
        assert!(!is_session_id("zzzz"));
    }

    #[test]
    fn message_line_matches_spacing_variants() {
        assert!(is_message_line(br#"{"type":"message","id":"1"}"#));
        assert!(is_message_line(br#"{"type" : "message"}"#));
        assert!(!is_message_line(br#"{"type":"summary"}"#));
        assert!(!is_message_line(br#"{"kind":"message"}"#));
    }
}
