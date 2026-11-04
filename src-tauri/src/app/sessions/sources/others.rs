//! WorkBuddy / CodeBuddy 之外的三个来源：Qoder 两版、ZCode、Trae。
//!
//! 这三个客户端的会话正文都在各自的 SQLite 里，账号维度各不相同：
//! Qoder 与 ZCode 的库**没有账号字段**（换号共用一份库），所以归属列留空、界面显示「本机」；
//! Trae 的对话存在云端，本地只有输入历史，于是给已安装的版本各放一行占位，
//! 状态标「云端」——比列表里空着更像「读到了，只是没有正文」。

use crate::app::clients::Roots;
use crate::app::sessions::sources::SourceInfo;
use crate::app::sessions::store::RawRow;
use crate::types::SessionState;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

const QODER: [(&str, &str, bool); 2] = [
    ("qoder_cn", "Qoder CN", false),
    ("qoder_intl", "Qoder 国际版", true),
];
const ZCODE: (&str, &str) = ("zcode", "ZCode");
const TRAE: [(&str, &str, bool); 2] =
    [("Trae CN", "Trae CN", true), ("Trae", "Trae 国际版", false)];

fn qoder_db(roots: &Roots, intl: bool) -> PathBuf {
    roots.qoder(intl).join("main.sqlite")
}

fn zcode_db(roots: &Roots) -> PathBuf {
    roots.zcode().join("cli").join("db").join("db.sqlite")
}

/// 一条 ZCode 会话在 `cli/` 下的全部文件。
///
/// 库行之外它还拆在四处：`agents/<id>/`、`artifacts/<id>/`、`exec/bash-startup/<id>`
/// 和 `rollout/model-io-<id>.jsonl`。只删库行的话这些会一直留在盘上，
/// 「彻底删除」就退化成「只删索引」——用户在资源管理器里一眼就能看见。
pub fn zcode_artifacts(roots: &Roots, id: &str) -> Vec<PathBuf> {
    let cli = roots.zcode().join("cli");
    let mut out = Vec::new();
    for dir in ["agents", "artifacts", "exec/bash-startup"] {
        let path = cli.join(dir).join(id);
        if path.exists() {
            out.push(path);
        }
    }
    // rollout 的文件名带前缀（model-io-<id>.jsonl），按名字扫一层，别写死前缀
    if let Ok(entries) = std::fs::read_dir(cli.join("rollout")) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().contains(id) {
                out.push(entry.path());
            }
        }
    }
    out
}

pub fn sources(roots: &Roots) -> Vec<SourceInfo> {
    let mut out = Vec::new();
    for (key, label, intl) in QODER {
        let path = qoder_db(roots, intl);
        let present = path.exists();
        out.push(SourceInfo {
            key: key.into(),
            label: label.into(),
            fp: file_fp(&path),
            present,
        });
    }
    let path = zcode_db(roots);
    out.push(SourceInfo {
        key: ZCODE.0.into(),
        label: ZCODE.1.into(),
        fp: file_fp(&path),
        present: path.exists(),
    });
    for (key, label, cn) in TRAE {
        let present = roots.trae(cn).is_dir();
        out.push(SourceInfo {
            key: key.into(),
            label: label.into(),
            fp: if present {
                "present".into()
            } else {
                "absent".into()
            },
            present,
        });
    }
    out
}

pub fn scan_one(roots: &Roots, key: &str, errors: &mut Vec<String>) -> Vec<RawRow> {
    if let Some((_, label, intl)) = QODER.iter().find(|(k, _, _)| *k == key) {
        return scan_qoder(&qoder_db(roots, *intl), key, label, errors);
    }
    if key == ZCODE.0 {
        return scan_zcode(&zcode_db(roots), errors);
    }
    if let Some((folder, label, _)) = TRAE.iter().find(|(k, _, _)| *k == key) {
        return vec![placeholder(folder, label)];
    }
    Vec::new()
}

fn file_fp(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(stat) => format!(
            "{}:{}",
            stat.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0),
            stat.len()
        ),
        Err(_) => "absent".into(),
    }
}

fn open_ro(path: &Path) -> Option<Connection> {
    if !path.exists() {
        return None;
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// 各家时间字段有的给毫秒有的给秒：小于 1e12 的按秒补三位，否则界面会显示 1970。
fn normalize_ms(value: i64) -> i64 {
    if value > 0 && value < 1_000_000_000_000 {
        value * 1000
    } else {
        value
    }
}

fn state_of(deleted: Option<i64>, archived: Option<i64>) -> SessionState {
    if deleted.is_some() {
        SessionState::Deleted
    } else if archived.unwrap_or(0) != 0 {
        SessionState::Archived
    } else {
        SessionState::Visible
    }
}

fn scan_qoder(path: &Path, key: &str, label: &str, errors: &mut Vec<String>) -> Vec<RawRow> {
    let Some(conn) = open_ro(path) else {
        if path.exists() {
            errors.push(format!("{label}：会话库打不开（客户端可能正在写入）"));
        }
        return Vec::new();
    };
    let mut counts: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT session_id, COUNT(*), COALESCE(SUM(LENGTH(payload_json)), 0)
         FROM chat_session_messages GROUP BY session_id",
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        }) {
            for (sid, msgs, size) in rows.flatten() {
                counts.insert(sid, (msgs, size));
            }
        }
    }
    let mut out = Vec::new();
    let query = "SELECT session_id, title, cwd, updated_at, archived, deleted_at
                 FROM chat_sessions WHERE COALESCE(session_kind, 'standard') = 'standard'
                 ORDER BY updated_at DESC";
    let Ok(mut stmt) = conn.prepare(query) else {
        errors.push(format!("{label}：chat_sessions 表读不出"));
        return out;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1).unwrap_or(None),
            row.get::<_, Option<String>>(2).unwrap_or(None),
            row.get::<_, Option<i64>>(3).unwrap_or(None),
            row.get::<_, Option<i64>>(4).unwrap_or(None),
            row.get::<_, Option<i64>>(5).unwrap_or(None),
        ))
    }) else {
        errors.push(format!("{label}：chat_sessions 行读不出"));
        return out;
    };
    for (sid, title, cwd, updated, archived, deleted) in rows.flatten() {
        let (msgs, size) = counts.get(&sid).copied().unwrap_or((0, 0));
        let mut row = RawRow::empty("qoder", key, label);
        row.session_id = sid;
        row.title = title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "（未命名会话）".into());
        row.cwd = cwd.unwrap_or_default();
        row.last_at = normalize_ms(updated.unwrap_or(0));
        row.msgs = msgs;
        row.size = size;
        row.state = state_of(deleted, archived);
        row.has_body = msgs > 0;
        out.push(row);
    }
    out
}

fn scan_zcode(path: &Path, errors: &mut Vec<String>) -> Vec<RawRow> {
    let Some(conn) = open_ro(path) else {
        if path.exists() {
            errors.push("ZCode：会话库打不开（客户端可能正在写入）".into());
        }
        return Vec::new();
    };
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT session_id, COUNT(*) FROM message GROUP BY session_id")
    {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }) {
            for (sid, msgs) in rows.flatten() {
                counts.insert(sid, msgs);
            }
        }
    }
    let mut out = Vec::new();
    let query = "SELECT id, title, directory, time_updated, time_deleted, time_archived
                 FROM session WHERE parent_id IS NULL ORDER BY time_updated DESC";
    let Ok(mut stmt) = conn.prepare(query) else {
        errors.push("ZCode：session 表读不出".into());
        return out;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1).unwrap_or(None),
            row.get::<_, Option<String>>(2).unwrap_or(None),
            row.get::<_, Option<i64>>(3).unwrap_or(None),
            row.get::<_, Option<i64>>(4).unwrap_or(None),
            row.get::<_, Option<i64>>(5).unwrap_or(None),
        ))
    }) else {
        errors.push("ZCode：session 行读不出".into());
        return out;
    };
    for (sid, title, directory, updated, deleted, archived) in rows.flatten() {
        let msgs = counts.get(&sid).copied().unwrap_or(0);
        let mut row = RawRow::empty("zcode", ZCODE.0, ZCODE.1);
        row.session_id = sid;
        row.title = title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "（未命名会话）".into());
        row.cwd = directory.unwrap_or_default();
        row.last_at = normalize_ms(updated.unwrap_or(0));
        row.msgs = msgs;
        // 库有几百 MB，逐条会话统计体积要把 blob 全读一遍：体积记 0，
        // 展示层把 0 归一为「—」（见 store::page 的映射）
        row.size = 0;
        row.state = state_of(deleted, archived);
        row.has_body = msgs > 0;
        out.push(row);
    }
    out
}

fn placeholder(folder: &str, label: &str) -> RawRow {
    let mut row = RawRow::empty("trae", folder, label);
    row.session_id = format!("cloud:{folder}");
    row.title = "对话记录存在云端，本地没有可管理的正文".into();
    row.state = SessionState::Cloud;
    row.has_body = false;
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_are_normalised() {
        assert_eq!(normalize_ms(1_760_000_000), 1_760_000_000_000);
        assert_eq!(normalize_ms(1_760_000_000_000), 1_760_000_000_000);
        assert_eq!(normalize_ms(0), 0);
    }

    #[test]
    fn states_follow_columns() {
        assert_eq!(state_of(Some(1), None), SessionState::Deleted);
        assert_eq!(state_of(None, Some(1)), SessionState::Archived);
        assert_eq!(state_of(None, None), SessionState::Visible);
    }
}
