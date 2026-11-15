//! 会话正文全文索引（FTS5 external-content）。
//!
//! 两张表（建表见 `db.rs`）：
//!   - `session_text` 是**唯一事实**：正文 + 指纹（fp），按 `(source_key, session_id)` 唯一；
//!   - `session_fts` 是它的 external-content 倒排，只存 body 一列，rowid 对齐。
//!
//! 写入纪律：**先写 text，再同步 fts，两边必须成对**。更新 = 改 text + 删旧 fts 行 +
//! 插新 fts 行；删除 = 按 rowid 删两边。读侧（搜索）见 `store.rs` 的 `body_clause`。
//!
//! 为什么不把正文存进 `session_index`：索引表每轮扫描整源重建，正文解析（要开客户端
//! 库、读 jsonl）比列表查询贵几个数量级，绝不能挂在列表的读写路径上。这里走独立的
//! 后台管线：扫描完成后按指纹挑出「变了或没索引过」的会话，每轮限量处理。

use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::sessions::preview::{self, Target};
use chrono::Local;

/// 单会话正文的字节上限：超出的尾部直接截掉（索引里少一段，比把库撑爆强）。
const BODY_BYTES_CAP: usize = 200_000;

/// 待索引行：索引表里"有正文指纹"的会话信息（fp 取 last_at + msgs，动了就重索引）。
struct Pending {
    kind: String,
    source_key: String,
    account: String,
    session_id: String,
    fp: String,
}

/// 挑出本轮要索引的会话：`session_text` 里没有、或指纹变了（会话活动过）。
fn pending(db: &Db, limit: usize) -> Result<Vec<Pending>> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT s.kind, s.source_key, s.account, s.session_id,
                    CAST(COALESCE(s.last_at, 0) AS TEXT) || ':' || CAST(COALESCE(s.msgs, 0) AS TEXT) AS fp
             FROM session_index s
             LEFT JOIN session_text t
               ON t.source_key = s.source_key AND t.session_id = s.session_id
             WHERE s.has_body = 1
               AND (t.rowid IS NULL OR t.fp <> (CAST(COALESCE(s.last_at, 0) AS TEXT) || ':' || CAST(COALESCE(s.msgs, 0) AS TEXT)))
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(Pending {
                kind: row.get(0)?,
                source_key: row.get(1)?,
                account: row.get(2)?,
                session_id: row.get(3)?,
                fp: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    })
}

/// 后台索引一批（限流由调用方控制，后台任务每轮 20 条）。
/// 返回本轮成功索引的条数。单条失败只记日志，不影响其他条。
pub fn index_batch(roots: &Roots, db: &Db, limit: usize) -> Result<usize> {
    let batch = pending(db, limit)?;
    let mut done = 0usize;
    for item in batch {
        let target = Target {
            kind: item.kind,
            source_key: item.source_key.clone(),
            account: item.account,
            session_id: item.session_id.clone(),
        };
        // 界面预览的头尾口径对搜索不够用，这里走全量（仍受 EXPORT_MAX / BODY cap 双重硬顶）
        let loaded = preview::load_for(roots, &target, true);
        let mut body = String::new();
        for message in &loaded.messages {
            if message.role == "gap" || message.text.is_empty() {
                continue;
            }
            body.push_str(&message.text);
            body.push('\n');
        }
        // 云端占位 / 加密库读不出正文的行不值得占索引：没有正文就不写
        if body.trim().is_empty() {
            continue;
        }
        if body.len() > BODY_BYTES_CAP {
            let mut cut = BODY_BYTES_CAP;
            while !body.is_char_boundary(cut) {
                cut -= 1;
            }
            body.truncate(cut);
        }
        let truncated = loaded.truncated || loaded.total > loaded.messages.len();
        write_one(
            db,
            &item.source_key,
            &item.session_id,
            &item.fp,
            truncated,
            &body,
        )?;
        done += 1;
    }
    if done > 0 {
        prune_gone(db)?;
    }
    Ok(done)
}

/// 写一行正文索引（text + fts 成对更新）。
pub(crate) fn write_one(
    db: &Db,
    source_key: &str,
    session_id: &str,
    fp: &str,
    truncated: bool,
    body: &str,
) -> Result<()> {
    db.write(|conn| {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT rowid FROM session_text WHERE source_key = ?1 AND session_id = ?2",
                rusqlite::params![source_key, session_id],
                |row| row.get(0),
            )
            .ok();
        match existing {
            Some(rowid) => {
                conn.execute(
                    "UPDATE session_text SET fp = ?1, truncated = ?2, body = ?3
                     WHERE rowid = ?4",
                    rusqlite::params![fp, truncated as i64, body, rowid],
                )?;
                // external-content 表不会自己感知内容变化：删旧插新
                conn.execute("DELETE FROM session_fts WHERE rowid = ?1", [rowid])?;
                conn.execute(
                    "INSERT INTO session_fts(rowid, body) VALUES (?1, ?2)",
                    rusqlite::params![rowid, body],
                )?;
            }
            None => {
                conn.execute(
                    "INSERT INTO session_text(source_key, session_id, fp, truncated, body)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![source_key, session_id, fp, truncated as i64, body],
                )?;
                let rowid = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO session_fts(rowid, body) VALUES (?1, ?2)",
                    rusqlite::params![rowid, body],
                )?;
            }
        }
        Ok(())
    })
}

/// 会话从索引表里消失（客户端删了 / 彻底删除 / 整源换掉）时，正文索引跟着摘掉。
/// 每轮索引完顺手清一次；彻底删除走 `forget_keys` 即时清。
pub(crate) fn prune_gone(db: &Db) -> Result<usize> {
    db.write(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t.rowid FROM session_text t
             WHERE NOT EXISTS (
                 SELECT 1 FROM session_index s
                 WHERE s.source_key = t.source_key AND s.session_id = t.session_id
             )",
        )?;
        let rowids: Vec<i64> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut removed = 0;
        for rowid in rowids {
            conn.execute("DELETE FROM session_fts WHERE rowid = ?1", [rowid])?;
            conn.execute("DELETE FROM session_text WHERE rowid = ?1", [rowid])?;
            removed += 1;
        }
        Ok(removed)
    })
}

/// 彻底删除后即时清掉正文索引（delete.rs 调用）。
pub fn forget_keys(db: &Db, keys: &[String]) -> Result<()> {
    for key in keys {
        let Ok(target) = preview::parse_key(key) else {
            continue;
        };
        db.write(|conn| {
            let rowid: Option<i64> = conn
                .query_row(
                    "SELECT rowid FROM session_text WHERE source_key = ?1 AND session_id = ?2",
                    rusqlite::params![target.source_key, target.session_id],
                    |row| row.get(0),
                )
                .ok();
            if let Some(rowid) = rowid {
                conn.execute("DELETE FROM session_fts WHERE rowid = ?1", [rowid])?;
                conn.execute("DELETE FROM session_text WHERE rowid = ?1", [rowid])?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// 索引全量重建（external-content 表与事实表错位时的自救出口：FTS 出脏结果时用）。
#[allow(dead_code)]
pub fn rebuild(db: &Db) -> Result<()> {
    db.write(|conn| {
        conn.execute("INSERT INTO session_fts(session_fts) VALUES('rebuild')", [])?;
        Ok(())
    })
}

/// 后台任务的节流开关（kv）：`session.fts_enabled = 0` 时管线空转。
pub const ENABLE_KEY: &str = "session.fts_enabled";

pub fn enabled(db: &Db) -> bool {
    db.kv_get(ENABLE_KEY)
        .ok()
        .flatten()
        .map(|v| v.trim() != "0")
        .unwrap_or(true)
}

/// 今天的日志用时间戳（本地时区秒级）。
#[allow(dead_code)]
fn stamp() -> String {
    Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::sessions::store::RawRow;

    fn seed(db: &Db, sid: &str, title: &str, last_at: i64, has_body: bool) {
        let mut row = RawRow::empty("qoder", "qoder_cn", "Qoder CN");
        row.session_id = sid.into();
        row.title = title.into();
        row.last_at = last_at;
        row.has_body = has_body;
        crate::app::sessions::store::SqliteSessions::new(db)
            .replace_source("qoder_cn", &[row])
            .unwrap();
    }

    #[test]
    fn write_search_and_prune_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "s1", "会话一", 10, true);
        write_one(&db, "qoder_cn", "s1", "10:1", false, "关于部署脚本的讨论").unwrap();

        // 搜得到：store 的 body_clause 走 session_fts
        let store = crate::app::sessions::store::SqliteSessions::new(&db);
        let filter = crate::app::sessions::store::Filter {
            body: Some("部署脚本".into()),
            ..Default::default()
        };
        let page = store.page(&filter, None, 100).unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.items[0]
            .snippet
            .as_deref()
            .unwrap_or("")
            .contains("部署脚本"));

        // 指纹变了（会话活动过）会再进候选
        seed(&db, "s1", "会话一", 20, true);
        let batch = pending(&db, 10).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].fp, "20:0", "fp = last_at:msgs");

        // 索引行消失后 prune 摘掉正文索引
        crate::app::sessions::store::SqliteSessions::new(&db)
            .drop_source("qoder_cn")
            .unwrap();
        assert_eq!(prune_gone(&db).unwrap(), 1);
        let filter = crate::app::sessions::store::Filter {
            body: Some("部署脚本".into()),
            ..Default::default()
        };
        assert_eq!(store.page(&filter, None, 100).unwrap().items.len(), 0);
    }

    #[test]
    fn forget_keys_removes_body_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "s1", "t", 1, true);
        write_one(&db, "qoder_cn", "s1", "1:1", false, "正文内容").unwrap();
        forget_keys(&db, &["qoder|qoder_cn||s1".to_string()]).unwrap();
        let left: i64 = db
            .read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM session_text", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(left, 0);
    }
}
