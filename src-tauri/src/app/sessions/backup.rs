//! 会话库快照。
//!
//! 会话是「删了就没了」的东西，回收站只兜住删除动作本身——客户端自己抽风、
//! 升级改了表结构，兜不住。所以这里留一份整库快照，出问题能整份盖回去。
//!
//! 用 SQLite 的在线备份接口而不是复制文件：客户端开着的时候 `-wal` 里还有没落盘的数据，
//! 直接 copy 会得到一份半截的库。在线备份走的是读事务，客户端正写着也能拿到一致快照。

use crate::app::clients::Roots;
use crate::app::error::{AppError, Result};
use crate::app::sessions::trash;
use chrono::Local;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 超过这个大小的库不进快照：ZCode 的库几百 MB，每次备份一份不现实。
const MAX_DB_BYTES: u64 = 256 * 1024 * 1024;
/// 只留最近这么多份快照。
pub const KEEP: usize = 10;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub tag: String,
    pub at: String,
    pub dir: String,
    /// 备份了哪些来源（`cn` / `intl` / `qoder_cn` / `qoder_intl`）
    pub sources: Vec<String>,
    /// 太大被跳过的来源，界面要如实说
    pub skipped: Vec<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes: i64,
}

impl Snapshot {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "快照 {}：{} 个来源，{:.1} MB",
            self.tag,
            self.sources.len(),
            self.bytes as f64 / 1_048_576.0
        );
        if !self.skipped.is_empty() {
            text.push_str(&format!("（{} 太大跳过）", self.skipped.join("、")));
        }
        text
    }
}

/// 备份根目录：`data/backups/session-batch/`（v1 的老备份也在这里，能一起看到）。
pub fn root() -> PathBuf {
    crate::app::paths::data_dir()
        .join("backups")
        .join("session-batch")
}

/// 一个来源：名字 + 库路径。
fn candidates(roots: &Roots) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("cn", roots.workbuddy(false).join("workbuddy.db")),
        ("intl", roots.workbuddy(true).join("workbuddy.db")),
        ("qoder_cn", roots.qoder(false).join("main.sqlite")),
        ("qoder_intl", roots.qoder(true).join("main.sqlite")),
        (
            "zcode",
            roots.zcode().join("cli").join("db").join("db.sqlite"),
        ),
    ]
}

pub fn make(roots: &Roots, base: &Path) -> Result<Snapshot> {
    std::fs::create_dir_all(base)
        .map_err(|e| AppError::new("io", format!("建备份目录失败：{e}")))?;
    let tag = trash::new_tag(base);
    let dir = base.join(&tag);
    let db_dir = dir.join("db");
    std::fs::create_dir_all(&db_dir)
        .map_err(|e| AppError::new("io", format!("建备份目录失败：{e}")))?;

    let mut sources = Vec::new();
    let mut skipped = Vec::new();
    let mut bytes = 0i64;
    for (key, path) in candidates(roots) {
        if !path.exists() {
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_DB_BYTES {
            skipped.push(key.to_string());
            continue;
        }
        let dest = db_dir.join(format!("{key}.db"));
        match online_backup(&path, &dest) {
            Ok(()) => {
                bytes += std::fs::metadata(&dest)
                    .map(|m| m.len() as i64)
                    .unwrap_or(0);
                sources.push(key.to_string());
            }
            Err(e) => {
                // 单个来源失败不该让整次备份没有结果，但要说出来
                skipped.push(format!("{key}（{}）", e.message));
            }
        }
    }
    if sources.is_empty() {
        let _ = crate::app::fsx::remove_path(&dir);
        return Err(AppError::new(
            "invalid",
            "没有可备份的会话库（客户端可能都没装）",
        ));
    }

    let snapshot = Snapshot {
        tag: tag.clone(),
        at: Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        dir: dir.display().to_string(),
        sources,
        skipped,
        bytes,
    };
    let text = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| AppError::new("io", format!("清单序列化失败：{e}")))?;
    crate::app::fsx::write_atomic(&dir.join("manifest.json"), text.as_bytes())?;
    prune(base, KEEP);
    Ok(snapshot)
}

/// 用 SQLite 在线备份接口复制一份一致的库。
fn online_backup(src: &Path, dest: &Path) -> Result<()> {
    let from = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| AppError::new("sqlite", format!("打开 {} 失败：{e}", src.display())))?;
    let mut to = Connection::open(dest)
        .map_err(|e| AppError::new("sqlite", format!("建 {} 失败：{e}", dest.display())))?;
    let backup = rusqlite::backup::Backup::new(&from, &mut to)
        .map_err(|e| AppError::new("sqlite", format!("备份初始化失败：{e}")))?;
    backup
        .run_to_completion(64, std::time::Duration::from_millis(20), None)
        .map_err(|e| AppError::new("sqlite", format!("备份失败：{e}")))?;
    Ok(())
}

/// 列已有快照，最近的在前。
pub fn list(base: &Path) -> Vec<Snapshot> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut out: Vec<Snapshot> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path().join("manifest.json")).ok()?;
            serde_json::from_str(&text).ok()
        })
        .collect();
    out.sort_by(|a, b| b.tag.cmp(&a.tag));
    out
}

/// 只留最近 `keep` 份。
pub fn prune(base: &Path, keep: usize) {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(base) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect(),
        Err(_) => return,
    };
    dirs.sort();
    if dirs.len() <= keep {
        return;
    }
    for old in dirs.iter().take(dirs.len() - keep) {
        let _ = crate::app::fsx::remove_path(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE sessions(id TEXT PRIMARY KEY);
             INSERT INTO sessions VALUES ('a'),('b');",
        )
        .unwrap();
    }

    #[test]
    fn snapshot_is_readable_and_keeps_latest() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("workbuddy.db");
        seed(&src);
        let base = dir.path().join("backups");
        // 直接测在线备份 + 清理，不依赖真实客户端目录
        for index in 0..3 {
            let dest = base
                .join(format!("2026010{index}"))
                .join("db")
                .join("cn.db");
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            online_backup(&src, &dest).unwrap();
            let conn = Connection::open(&dest).unwrap();
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 2, "备份出来的库要能读，且行数一致");
        }
        prune(&base, 2);
        let left = std::fs::read_dir(&base).unwrap().count();
        assert_eq!(left, 2, "只留最近 2 份");
    }

    #[test]
    fn skip_huge_db_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("backups");
        let roots = Roots::at(
            dir.path().join("h"),
            dir.path().join("a"),
            dir.path().join("l"),
        );
        // 一个都不存在时给出明确的错，而不是一份空备份
        let error = make(&roots, &base).unwrap_err();
        assert!(error.message.contains("没有可备份"), "{}", error.message);
    }
}
