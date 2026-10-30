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
        let write = open_conn(&path)?;
        let read = open_conn(&path)?;
        let db = Self { write: Mutex::new(write), read: Mutex::new(read) };
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
                 );",
            )?;
            // execute_batch 不支持 ?1 占位符，首行只能单独插
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM schema_meta", [], |r| r.get(0))?;
            if count == 0 {
                conn.execute("INSERT INTO schema_meta(version) VALUES (?1)", [SCHEMA_VERSION])?;
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
        let now = now_ms();
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
                conn.query_row("SELECT migrated_from FROM schema_meta LIMIT 1", [], |r| r.get(0))?;
            Ok(value)
        })
    }

    pub fn mark_migrated(&self, from: &str) -> Result<()> {
        self.write(|conn| {
            conn.execute(
                "UPDATE schema_meta SET migrated_from = ?1, migrated_at = ?2",
                rusqlite::params![from, now_ms()],
            )?;
            Ok(())
        })
    }
}

fn open_conn(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .map_err(|e| AppError::new("sqlite", format!("打开数据库失败：{e}")))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

fn poisoned() -> AppError {
    AppError::new("sqlite", "数据库锁被污染（上一次操作 panic 过）")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
        assert_eq!(db.kv_get("theme").unwrap().as_deref(), Some("light"), "kv_set 必须是 upsert");
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
