//! 应用数据库。`data/app.db`
//!
//! **1 写连接 + 1 读连接**，不用连接池（并发量级不需要），但必须读写分离：
//! 单连接会把所有读写串成一条线，备份/迁移这类长事务会连任务页的刷新一起卡住。
//! WAL 下读写本就并发，代价只有约 20 行。

use crate::app::error::{AppError, Result};
use crate::app::paths;
use rusqlite::Connection;
use std::sync::Mutex;

const SCHEMA_VERSION: u32 = 1;

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
                 -- 业务表在 P1 一次建齐：v1 导入器必须有落点。
                 -- 后续阶段只准「加列/加表」并升级 SCHEMA_VERSION，不准重建。
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
                // 上面注释承诺「升级 SCHEMA_VERSION」：版本错配时带病运行比启动期报错糟
                let stored: i64 =
                    conn.query_row("SELECT MAX(version) FROM schema_meta", [], |r| r.get(0))?;
                if stored != SCHEMA_VERSION as i64 {
                    return Err(AppError::new(
                        "sqlite",
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

    pub fn schema_version(&self) -> Result<u32> {
        self.read(|conn| {
            let version: u32 =
                conn.query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))?;
            Ok(version)
        })
    }

    /// v1 导入的幂等守卫：已导入则拒绝重跑。
    pub fn migrated_from(&self) -> Result<Option<String>> {
        self.read(|conn| {
            let value: Option<String> =
                conn.query_row("SELECT migrated_from FROM schema_meta LIMIT 1", [], |r| {
                    r.get(0)
                })?;
            Ok(value)
        })
    }

    pub fn mark_migrated(&self, from: &str) -> Result<()> {
        self.write(|conn| {
            conn.execute(
                "UPDATE schema_meta SET migrated_from = ?1, migrated_at = ?2",
                rusqlite::params![from, crate::app::timeutil::now_ms()],
            )?;
            Ok(())
        })
    }
}

fn open_conn(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .map_err(|e| AppError::new("sqlite", format!("打开数据库失败：{e}")))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
    )?;
    Ok(conn)
}

fn poisoned() -> AppError {
    AppError::new("sqlite", "数据库锁被污染（上一次操作 panic 过）")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(db.migrated_from().unwrap(), None);
    }

    #[test]
    fn migration_guard_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        db.mark_migrated("v1.4.0").unwrap();
        assert_eq!(db.migrated_from().unwrap().as_deref(), Some("v1.4.0"));
    }
}
