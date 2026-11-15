//! 会话的本地标记：归档 / 本地隐藏 / 置顶 / 本地命名 / 标签。
//!
//! 为什么单独一张表：`session_index` 是**整源重建**的（扫描完一个来源就删掉重插），
//! 任何用户态挂在索引行上都会被下一轮扫描抹掉。归档时刻、隐藏墓碑、置顶、改名
//! 必须活在索引之外，用 `(source_key, session_id)` 关联；标签是多对多，独立成
//! `session_tag` 表。扫描器对这里一无所知，`replace_source` 也不碰这些表。
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
                prune_empty(conn, &target.source_key, &target.session_id);
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
            conn.execute(
                "DELETE FROM session_tag WHERE source_key = ?1 AND session_id = ?2",
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

// ------------------------------------------------------------------ v3：置顶 / 改名 / 取消隐藏 / 标签

/// 置顶 / 取消置顶。置顶只影响本工具的列表排序，客户端毫无感知。
pub fn pin(db: &Db, keys: &[String], on: bool) -> Result<usize> {
    let now = stamp();
    let mut changed = 0;
    for key in keys {
        let target = preview::parse_key(key)?;
        db.write(|conn| {
            if on {
                conn.execute(
                    "INSERT INTO session_marks(source_key, session_id, pinned_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(source_key, session_id) DO UPDATE SET pinned_at = ?3",
                    rusqlite::params![target.source_key, target.session_id, now],
                )?;
            } else {
                conn.execute(
                    "UPDATE session_marks SET pinned_at = NULL
                     WHERE source_key = ?1 AND session_id = ?2",
                    rusqlite::params![target.source_key, target.session_id],
                )?;
                prune_empty(conn, &target.source_key, &target.session_id);
            }
            Ok(())
        })?;
        changed += 1;
    }
    Ok(changed)
}

/// 本地改名。`Some(title)` 写覆盖标题（**锁定**：扫描永不覆盖它）；
/// `None` = 恢复客户端原标题（清掉覆盖）。空串视同恢复原标题。
pub fn rename(db: &Db, key: &str, title: Option<&str>) -> Result<()> {
    let target = preview::parse_key(key)?;
    let title = title.map(str::trim).filter(|s| !s.is_empty());
    db.write(|conn| {
        match title {
            Some(title) => {
                conn.execute(
                    "INSERT INTO session_marks(source_key, session_id, title_override)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(source_key, session_id) DO UPDATE SET title_override = ?3",
                    rusqlite::params![target.source_key, target.session_id, title],
                )?;
            }
            None => {
                conn.execute(
                    "UPDATE session_marks SET title_override = NULL
                     WHERE source_key = ?1 AND session_id = ?2",
                    rusqlite::params![target.source_key, target.session_id],
                )?;
                prune_empty(conn, &target.source_key, &target.session_id);
            }
        }
        Ok(())
    })
}

/// 取消本地隐藏。Trae 家族的「彻底删除」就是写墓碑（远端删不掉），
/// 这里放开的是同一个标记——确认弹窗要说清「等于恢复显示」。
pub fn unhide(db: &Db, keys: &[String]) -> Result<usize> {
    let mut changed = 0;
    for key in keys {
        let target = preview::parse_key(key)?;
        db.write(|conn| {
            conn.execute(
                "UPDATE session_marks SET hidden_at = NULL
                 WHERE source_key = ?1 AND session_id = ?2",
                rusqlite::params![target.source_key, target.session_id],
            )?;
            prune_empty(conn, &target.source_key, &target.session_id);
            Ok(())
        })?;
        changed += 1;
    }
    Ok(changed)
}

/// 全量替换一个会话的标签。空标签与重复项在写入前压掉。
pub fn tags_set(db: &Db, key: &str, tags: &[String]) -> Result<usize> {
    let target = preview::parse_key(key)?;
    // 有序去重：界面按添加顺序展示，BTreeSet 会打乱，手动去重保序
    let mut clean: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() || clean.iter().any(|t| t == tag) {
            continue;
        }
        clean.push(tag.to_string());
    }
    db.write(|conn| {
        conn.execute(
            "DELETE FROM session_tag WHERE source_key = ?1 AND session_id = ?2",
            rusqlite::params![target.source_key, target.session_id],
        )?;
        for tag in &clean {
            conn.execute(
                "INSERT OR IGNORE INTO session_tag(source_key, session_id, tag)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![target.source_key, target.session_id, tag],
            )?;
        }
        Ok(())
    })?;
    Ok(clean.len())
}

/// 两个标记都空、也没有置顶与改名的行没有存在意义。
/// 归档 / 取消隐藏 / 取消置顶 / 恢复原标题共用；标签在独立表里，不参与判断。
fn prune_empty(conn: &rusqlite::Connection, source_key: &str, session_id: &str) {
    let _ = conn.execute(
        "DELETE FROM session_marks
         WHERE source_key = ?1 AND session_id = ?2
           AND archived_at IS NULL AND hidden_at IS NULL
           AND pinned_at IS NULL AND title_override IS NULL",
        rusqlite::params![source_key, session_id],
    );
}

/// 到点自动归档：最后活动早于 `days` 天的会话打上归档标记，返回新增条数。
/// `days = 0` 表示关闭（用户选「不自动归档」）。
///
/// 判定依据是 `last_at`（**最后活动时间**，也就是列表里显示的时间），不是创建时间：
/// 归档要回答的是「很久没动过了」，一个天天在用的老会话当然不该被收走。
///
/// 覆盖范围是「还会出现在主列表里的行」——除了客户端已删除（`deleted`，那类直接彻底
/// 删除即可）之外的 `visible` / `orphan` / `cloud` / `old_account` 都算。早先只认
/// `visible`，结果 ZCode 那批父会话已消失的 `orphan` 行永远留在列表里，看起来像
/// 「自动归档没生效」。
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
             WHERE COALESCE(s.state, 'visible') <> 'deleted'
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
        // 孤儿（父会话已消失的内部会话）同样会出现在主列表里，老的也要收走
        seed(&db, "qoder_cn", "orphan", old, "orphan");
        // 云端占位同理：列表里能看到，就该跟着归档
        seed(&db, "qoder_cn", "cloud", old, "cloud");
        assert_eq!(auto_archive(&db, 30).unwrap(), 3);
        assert!(is_archived(&db, "qoder|qoder_cn||old").unwrap());
        assert!(!is_archived(&db, "qoder|qoder_cn||new").unwrap());
        assert!(!is_archived(&db, "qoder|qoder_cn||gone").unwrap());
        assert!(is_archived(&db, "qoder|qoder_cn||orphan").unwrap());
        assert!(is_archived(&db, "qoder|qoder_cn||cloud").unwrap());
        // 关掉自动归档就不动任何东西
        assert_eq!(auto_archive(&db, 0).unwrap(), 0);
    }

    /// 置顶：unpin 之后行被修剪掉（不留空壳）；自动归档照常给置顶的会话打归档。
    #[test]
    fn pin_roundtrip_and_prune() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "qoder_cn", "s1", 1, "visible");
        let key = "qoder|qoder_cn||s1".to_string();
        pin(&db, std::slice::from_ref(&key), true).unwrap();
        // 置顶行存在
        let rows: i64 = db
            .read(
                |conn| Ok(conn.query_row("SELECT COUNT(*) FROM session_marks", [], |r| r.get(0))?),
            )
            .unwrap();
        assert_eq!(rows, 1);
        pin(&db, std::slice::from_ref(&key), false).unwrap();
        let rows: i64 = db
            .read(
                |conn| Ok(conn.query_row("SELECT COUNT(*) FROM session_marks", [], |r| r.get(0))?),
            )
            .unwrap();
        assert_eq!(rows, 0, "取消置顶后空壳行要被修剪");
    }

    /// 改名：写覆盖 → 扫描整源重建（replace 走的同一张索引表）不丢；
    /// 恢复原标题 → 覆盖清掉、空壳行修剪。空串视同恢复。
    #[test]
    fn rename_override_survives_rescan() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "qoder_cn", "s1", 1, "visible");
        let key = "qoder|qoder_cn||s1".to_string();
        rename(&db, &key, Some("我的命名")).unwrap();
        // 模拟一轮扫描更新了这行（marks 与索引行互不相干，不该被波及）
        db.write(|conn| {
            conn.execute(
                "UPDATE session_index SET last_at = 2 WHERE session_id = 's1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let title: String = db
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT title_override FROM session_marks
                     WHERE source_key = 'qoder_cn' AND session_id = 's1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(title, "我的命名");
        rename(&db, &key, Some("")).unwrap();
        let rows: i64 = db
            .read(
                |conn| Ok(conn.query_row("SELECT COUNT(*) FROM session_marks", [], |r| r.get(0))?),
            )
            .unwrap();
        assert_eq!(rows, 0, "空串 = 恢复原标题，空壳行修剪");
    }

    /// 取消隐藏与标签：unhide 清墓碑；tags_set 全量替换、去重、去空。
    #[test]
    fn unhide_and_tags_replace() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "qoder_cn", "s1", 1, "visible");
        let key = "qoder|qoder_cn||s1".to_string();
        hide(&db, std::slice::from_ref(&key)).unwrap();
        unhide(&db, std::slice::from_ref(&key)).unwrap();
        let rows: i64 = db
            .read(
                |conn| Ok(conn.query_row("SELECT COUNT(*) FROM session_marks", [], |r| r.get(0))?),
            )
            .unwrap();
        assert_eq!(rows, 0, "取消隐藏后空壳行被修剪");

        let tags = vec!["工作".into(), " 工作 ".into(), String::new(), "重要".into()];
        assert_eq!(tags_set(&db, &key, &tags).unwrap(), 2, "去空、去重");
        let count: i64 = db
            .read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM session_tag", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(count, 2);
        tags_set(&db, &key, &[]).unwrap();
        let count: i64 = db
            .read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM session_tag", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(count, 0, "全量替换：传空数组即清空");
    }
}
