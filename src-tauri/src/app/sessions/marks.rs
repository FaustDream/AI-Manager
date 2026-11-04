//! 会话的本地标记：归档 / 本地隐藏。
//!
//! 为什么单独一张表：`session_index` 是**整源重建**的（扫描完一个来源就删掉重插），
//! 任何用户态挂在索引行上都会被下一轮扫描抹掉。归档时刻与「远端删不掉、只能本地
//! 隐藏」的墓碑都必须活在索引之外，用 `(source_key, session_id)` 关联。
//!
//! 归档只存在本工具里，不写客户端库：自动归档是按时间跑的，为了写一个标记去关掉
//! 用户正在用的客户端不划算，也会让客户端里的「归档」语义被本工具悄悄改掉。

use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::sessions::preview;
use chrono::{Local, SecondsFormat};

/// 本地隐藏：远端删不掉（只有云端占位）的会话，删一次就别再出现在列表里。
pub fn hide(db: &Db, keys: &[String]) -> Result<usize> {
    let now = stamp();
    let mut changed = 0;
    for key in keys {
        let target = preview::parse_key(key)?;
        db.write(|conn| {
            conn.execute(
                "INSERT INTO session_marks(source_key, session_id, hidden_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(source_key, session_id) DO UPDATE SET hidden_at = ?3",
                rusqlite::params![target.source_key, target.session_id, now],
            )?;
            Ok(())
        })?;
        changed += 1;
    }
    Ok(changed)
}

/// 归档 / 取消归档。`on = false` 只清归档时刻，不动隐藏标记。
pub fn archive(db: &Db, keys: &[String], on: bool) -> Result<usize> {
    let now = stamp();
    let mut changed = 0;
    for key in keys {
        let target = preview::parse_key(key)?;
        db.write(|conn| {
            if on {
                conn.execute(
                    "INSERT INTO session_marks(source_key, session_id, archived_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(source_key, session_id) DO UPDATE SET archived_at = ?3",
                    rusqlite::params![target.source_key, target.session_id, now],
                )?;
            } else {
                conn.execute(
                    "UPDATE session_marks SET archived_at = NULL
                     WHERE source_key = ?1 AND session_id = ?2",
                    rusqlite::params![target.source_key, target.session_id],
                )?;
                // 两个标记都空了的行没有存在意义
                conn.execute(
                    "DELETE FROM session_marks
                     WHERE source_key = ?1 AND session_id = ?2
                       AND archived_at IS NULL AND hidden_at IS NULL",
                    rusqlite::params![target.source_key, target.session_id],
                )?;
            }
            Ok(())
        })?;
        changed += 1;
    }
    Ok(changed)
}

/// 会话被真删掉之后清掉它的标记，否则同一 session_id 再出现会带着旧状态。
pub fn forget(db: &Db, keys: &[String]) -> Result<()> {
    for key in keys {
        let target = preview::parse_key(key)?;
        db.write(|conn| {
            conn.execute(
                "DELETE FROM session_marks WHERE source_key = ?1 AND session_id = ?2",
                rusqlite::params![target.source_key, target.session_id],
            )?;
            Ok(())
        })?;
    }
    Ok(())
}

/// 这个会话被归档了吗（`deleted` 优先级更高，由调用方判）。
pub fn is_archived(db: &Db, key: &str) -> Result<bool> {
    let target = preview::parse_key(key)?;
    db.read(|conn| {
        let hit: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM session_marks
                 WHERE source_key = ?1 AND session_id = ?2 AND archived_at IS NOT NULL",
                rusqlite::params![target.source_key, target.session_id],
                |row| row.get(0),
            )
            .ok();
        Ok(hit.is_some())
    })
}

/// 到点自动归档：最后活动早于 `days` 天的可见会话打上归档标记，返回新增条数。
/// `days = 0` 表示关闭（用户选「不自动归档」）。
pub fn auto_archive(db: &Db, days: u32) -> Result<usize> {
    if days == 0 {
        return Ok(0);
    }
    let cutoff = Local::now().timestamp_millis() - i64::from(days) * 24 * 3600 * 1000;
    let now = stamp();
    db.write(|conn| {
        let changed = conn.execute(
            "INSERT OR IGNORE INTO session_marks(source_key, session_id, archived_at)
             SELECT s.source_key, s.session_id, ?2
             FROM session_index s
             LEFT JOIN session_marks m
               ON m.source_key = s.source_key AND m.session_id = s.session_id
             WHERE COALESCE(s.state, 'visible') = 'visible'
               AND s.last_at IS NOT NULL AND s.last_at > 0 AND s.last_at < ?1
               AND m.archived_at IS NULL",
            rusqlite::params![cutoff, now],
        )?;
        Ok(changed)
    })
}

/// 归档保留天数（`0` = 关闭）。存在 kv 里，界面在会话页顶部直接改。
pub const RETENTION_KEY: &str = "session.archive_days";

pub fn retention_days(db: &Db) -> u32 {
    db.kv_get(RETENTION_KEY)
        .ok()
        .flatten()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

pub fn set_retention_days(db: &Db, days: u32) -> Result<()> {
    // 只认界面提供的四档 + 关闭，别让手改配置写出一个「2 天」的怪值
    let days = match days {
        3 | 7 | 15 | 30 => days,
        _ => 0,
    };
    db.kv_set(RETENTION_KEY, &days.to_string())
}

fn stamp() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(db: &Db, source: &str, sid: &str, last_at: i64, state: &str) {
        db.write(|conn| {
            conn.execute(
                "INSERT INTO session_index
                 (kind, source_key, source_label, account, session_id, title, last_at, state)
                 VALUES ('qoder', ?1, 'Qoder', '', ?2, 't', ?3, ?4)",
                rusqlite::params![source, sid, last_at, state],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn archive_and_unarchive_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "qoder_cn", "s1", 1, "visible");
        let key = "qoder|qoder_cn||s1".to_string();
        assert!(!is_archived(&db, &key).unwrap());
        archive(&db, std::slice::from_ref(&key), true).unwrap();
        assert!(is_archived(&db, &key).unwrap());
        archive(&db, std::slice::from_ref(&key), false).unwrap();
        assert!(!is_archived(&db, &key).unwrap());
    }

    #[test]
    fn auto_archive_only_touches_old_visible_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let now = Local::now().timestamp_millis();
        let old = now - 40 * 24 * 3600 * 1000;
        seed(&db, "qoder_cn", "old", old, "visible");
        seed(&db, "qoder_cn", "new", now, "visible");
        seed(&db, "qoder_cn", "gone", old, "deleted");
        assert_eq!(auto_archive(&db, 30).unwrap(), 1);
        assert!(is_archived(&db, "qoder|qoder_cn||old").unwrap());
        assert!(!is_archived(&db, "qoder|qoder_cn||new").unwrap());
        assert!(!is_archived(&db, "qoder|qoder_cn||gone").unwrap());
        // 关掉自动归档就不动任何东西
        assert_eq!(auto_archive(&db, 0).unwrap(), 0);
    }
}
