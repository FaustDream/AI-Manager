//! 应用数据库。`data/app.db`
//!
//! **1 写连接 + 1 读连接**，不用连接池（并发量级不需要），但必须读写分离：
//! 单连接会把所有读写串成一条线，备份/迁移这类长事务会连任务页的刷新一起卡住。
//! WAL 下读写本就并发，代价只有约 20 行。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::paths;
use rusqlite::Connection;
use std::sync::Mutex;

const SCHEMA_VERSION: u32 = 2;

pub struct Db {
    write: Mutex<Connection>,
    read: Mutex<Connection>,
}

impl Db {
    pub fn open() -> Result<Self> {
        Self::open_at(&paths::app_db())
    }

    /// 测试用：直接指定库路径，不读全局 data_dir，避免用例之间互相踩。
    pub fn open_at(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let write = open_conn(path)?;
        let read = open_conn(path)?;
        let db = Self {
            write: Mutex::new(write),
            read: Mutex::new(read),
        };
        db.bootstrap()?;
        Ok(db)
    }

    fn bootstrap(&self) -> Result<()> {
        self.write(|conn| {
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA foreign_keys=ON;
                 CREATE TABLE IF NOT EXISTS kv (
                     key        TEXT PRIMARY KEY,
                     value      TEXT NOT NULL,
                     updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS schema_meta (
                     version       INTEGER NOT NULL,
                     migrated_from TEXT,
                     migrated_at   INTEGER
                 );
                 -- 新版本直接建齐；不兼容旧 schema，也不再做旧库迁移。
                 CREATE TABLE IF NOT EXISTS accounts (
                     iid              TEXT PRIMARY KEY,
                     platform         TEXT NOT NULL,
                     platform_label   TEXT NOT NULL,
                     account_id       TEXT NOT NULL,
                     name             TEXT NOT NULL,
                     expires_at       INTEGER,
                     source           TEXT NOT NULL DEFAULT '',
                     requires_checkin INTEGER NOT NULL DEFAULT 1,
                     meta_keys        TEXT NOT NULL DEFAULT '[]',
                     secret_blob      BLOB,
                     enabled          INTEGER NOT NULL DEFAULT 1,
                     last_ok_at       INTEGER,
                     last_fail_at     INTEGER,
                     last_fail_kind   TEXT,
                     sort_order       INTEGER NOT NULL DEFAULT 0,
                     created_at       INTEGER NOT NULL,
                     updated_at       INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS checkin_log (
                     iid        TEXT NOT NULL,
                     checked_at INTEGER NOT NULL,
                     UNIQUE(iid, checked_at)
                 );
                 CREATE TABLE IF NOT EXISTS daily_marks (
                     iid  TEXT NOT NULL,
                     date TEXT NOT NULL,
                     PRIMARY KEY (iid, date)
                 );
                 CREATE TABLE IF NOT EXISTS credit_lots (
                     id         INTEGER PRIMARY KEY AUTOINCREMENT,
                     iid        TEXT NOT NULL,
                     amount     REAL NOT NULL,
                     expires_at INTEGER,
                     granted_at INTEGER,
                     lot_key    TEXT NOT NULL UNIQUE
                 );
                 CREATE TABLE IF NOT EXISTS credit_claims (
                     iid         TEXT NOT NULL,
                     event_key   TEXT NOT NULL,
                     source      TEXT NOT NULL,
                     amount      REAL,
                     description TEXT NOT NULL,
                     claimed_at  INTEGER NOT NULL,
                     PRIMARY KEY (iid, event_key)
                 );
                 CREATE INDEX IF NOT EXISTS idx_credit_claims_recent
                     ON credit_claims(iid, claimed_at DESC, event_key DESC);
                 CREATE TABLE IF NOT EXISTS token_usage_records (
                     record_id         TEXT PRIMARY KEY,
                     platform          TEXT NOT NULL,
                     model             TEXT NOT NULL,
                     prompt_tokens     INTEGER NOT NULL DEFAULT 0,
                     completion_tokens INTEGER NOT NULL DEFAULT 0,
                     total_tokens      INTEGER NOT NULL DEFAULT 0,
                     credit            REAL,
                     created_at        INTEGER NOT NULL,
                     source            TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_token_usage_time
                     ON token_usage_records(created_at DESC);
                 CREATE INDEX IF NOT EXISTS idx_token_usage_model
                     ON token_usage_records(model);
                 CREATE TABLE IF NOT EXISTS credit_snapshots (
                     iid     TEXT NOT NULL,
                     date    TEXT NOT NULL,
                     balance REAL NOT NULL,
                     PRIMARY KEY (iid, date)
                 );
                 CREATE TABLE IF NOT EXISTS session_index (
                     kind         TEXT NOT NULL,
                     source_key   TEXT NOT NULL,
                     source_label TEXT NOT NULL,
                     account      TEXT NOT NULL,
                     session_id   TEXT NOT NULL,
                     title        TEXT NOT NULL DEFAULT '',
                     slug         TEXT NOT NULL DEFAULT '',
                     cwd          TEXT NOT NULL DEFAULT '',
                     origin       TEXT NOT NULL DEFAULT '',
                     refs         TEXT NOT NULL DEFAULT '[]',
                     last_at      INTEGER,
                     seen_at      TEXT,
                     msgs         INTEGER,
                     size         INTEGER,
                     state        TEXT,
                     has_body     INTEGER,
                     PRIMARY KEY (kind, source_key, account, session_id)
                 );
                 CREATE INDEX IF NOT EXISTS idx_sessions_cursor
                     ON session_index(last_at DESC, session_id);
                 -- 会话的本地标记：归档与「本地隐藏」都只存在本工具里。
                 -- 索引表每轮扫描整源重建，所以这类用户态必须单独存，不能挂在索引行上。
                 --   archived_at  归档时刻（到点自动归档 / 手动归档）
                 --   hidden_at    本地隐藏（Trae 这类只有云端占位、远端删不掉的来源）
                 CREATE TABLE IF NOT EXISTS session_marks (
                     source_key  TEXT NOT NULL,
                     session_id  TEXT NOT NULL,
                     archived_at TEXT,
                     hidden_at   TEXT,
                     PRIMARY KEY (source_key, session_id)
                 );
                 CREATE TABLE IF NOT EXISTS hub_items (
                     kind       TEXT NOT NULL,
                     name       TEXT NOT NULL,
                     targets    TEXT NOT NULL DEFAULT '[]',
                     deployed   TEXT NOT NULL DEFAULT '{}',
                     digest     TEXT NOT NULL DEFAULT '',
                     updated_at INTEGER NOT NULL,
                     PRIMARY KEY (kind, name)
                 );",
            )?;
            // execute_batch 不支持 ?1 占位符，首行只能单独插
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM schema_meta", [], |r| r.get(0))?;
            if count == 0 {
                conn.execute(
                    "INSERT INTO schema_meta(version) VALUES (?1)",
                    [SCHEMA_VERSION],
                )?;
            } else {
                // 不做旧库迁移：所有环境都已升级到当前版本，版本不一致直接报错，
                // 避免带着旧结构运行产生难以排查的数据问题。
                let stored: i64 =
                    conn.query_row("SELECT MAX(version) FROM schema_meta", [], |r| r.get(0))?;
                if stored != SCHEMA_VERSION as i64 {
                    return Err(AppError::new(
                        ErrorKind::Sqlite,
                        format!("库 schema 版本不匹配：库是 v{stored}，程序要 v{SCHEMA_VERSION}"),
                    ));
                }
            }
            Ok(())
        })
    }

    /// 写操作。调用方必须把它包在 spawn_blocking 里（见架构 §2）。
    pub fn write<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.write.lock().map_err(|_| poisoned())?;
        f(&guard)
    }

    /// 读操作。同样需要 spawn_blocking。
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.read.lock().map_err(|_| poisoned())?;
        f(&guard)
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        self.read(|conn| {
            let mut stmt = conn.prepare("SELECT value FROM kv WHERE key = ?1")?;
            let mut rows = stmt.query([key])?;
            match rows.next()? {
                Some(row) => Ok(Some(row.get(0)?)),
                None => Ok(None),
            }
        })
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        let now = crate::app::timeutil::now_ms();
        self.write(|conn| {
            conn.execute(
                "INSERT INTO kv(key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                                updated_at = excluded.updated_at",
                rusqlite::params![key, value, now],
            )?;
            Ok(())
        })
    }

    pub fn kv_delete(&self, key: &str) -> Result<()> {
        self.write(|conn| {
            conn.execute("DELETE FROM kv WHERE key = ?1", [key])?;
            Ok(())
        })
    }

    /// 全部键值对（按 key 排序）。诊断包与配置展示用，调用方自己决定脱敏口径。
    pub fn kv_entries(&self) -> Result<Vec<(String, String)>> {
        self.read(|conn| {
            let mut stmt = conn.prepare("SELECT key, value FROM kv ORDER BY key")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn schema_version(&self) -> Result<u32> {
        self.read(|conn| {
            let version: u32 =
                conn.query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))?;
            Ok(version)
        })
    }
}

fn open_conn(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("打开数据库失败：{e}")))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
    )?;
    Ok(conn)
}

fn poisoned() -> AppError {
    AppError::new(ErrorKind::Sqlite, "数据库锁被污染（上一次操作 panic 过）")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_v2_database_gains_empty_claim_history_without_version_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("app.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_meta(version INTEGER NOT NULL);
             INSERT INTO schema_meta(version) VALUES (2);",
        )
        .unwrap();
        drop(conn);
        let db = Db::open_at(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), 2);
        assert!(crate::app::checkin::SqliteCheckin::new(&db)
            .claims("workbuddy:u1")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn kv_roundtrip_and_schema_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();

        assert_eq!(db.kv_get("nope").unwrap(), None);
        db.kv_set("theme", "dark").unwrap();
        assert_eq!(db.kv_get("theme").unwrap().as_deref(), Some("dark"));
        db.kv_set("theme", "light").unwrap();
        assert_eq!(
            db.kv_get("theme").unwrap().as_deref(),
            Some("light"),
            "kv_set 必须是 upsert"
        );
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }
}
