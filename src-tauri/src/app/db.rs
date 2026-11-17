//! 应用数据库。`data/app.db`
//!
//! **1 写连接 + 1 读连接**，不用连接池（并发量级不需要），但必须读写分离：
//! 单连接会把所有读写串成一条线，备份/迁移这类长事务会连任务页的刷新一起卡住。
//! WAL 下读写本就并发，代价只有约 20 行。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::paths;
use rusqlite::Connection;
use std::sync::Mutex;

pub(crate) const SCHEMA_VERSION: u32 = 4;

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
                 -- 签到提交标记：reserved=1 表示「今天已提交领取、结果未确认」。
                 -- 这是跨轮次幂等的第二道防线（第一道 daily_marks 记「已签成功」）：
                 -- 一旦提交，同一天后续轮次只回查、不重复领取，避免「响应丢失→重领」。
                 CREATE TABLE IF NOT EXISTS checkin_attempts (
                     iid      TEXT NOT NULL,
                     date     TEXT NOT NULL,
                     reserved INTEGER NOT NULL DEFAULT 0,
                     at       INTEGER NOT NULL,
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
                     source_key     TEXT NOT NULL,
                     session_id     TEXT NOT NULL,
                     archived_at    TEXT,
                     hidden_at      TEXT,
                     -- v3 会话管理扩展：置顶与本地命名。索引表每轮扫描整源重建，
                     -- 这类用户态只能活在 marks（与 archived/hidden 同一条铁律）
                     pinned_at      TEXT,
                     title_override TEXT,
                     PRIMARY KEY (source_key, session_id)
                 );
                 -- 标签：一个会话可挂多个，独立成表才能按 tag 建索引、按 tag 聚合计数
                 CREATE TABLE IF NOT EXISTS session_tag (
                     source_key TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     tag        TEXT NOT NULL,
                     PRIMARY KEY (source_key, session_id, tag)
                 );
                 CREATE INDEX IF NOT EXISTS idx_session_tag_tag ON session_tag(tag);
                 -- 正文全文索引（v3）：session_text 是唯一事实（正文 + 指纹），
                 -- session_fts 是它的 external-content FTS5 倒排（rowid 对齐，body 不重复存）。
                 -- 写入纪律见 sessions/fts.rs：先写 text 再同步 fts，删除两边一起删。
                 CREATE TABLE IF NOT EXISTS session_text (
                     source_key TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     fp         TEXT NOT NULL DEFAULT '',
                     truncated  INTEGER NOT NULL DEFAULT 0,
                     body       TEXT NOT NULL DEFAULT '',
                     UNIQUE (source_key, session_id)
                 );
                 CREATE VIRTUAL TABLE IF NOT EXISTS session_fts USING fts5(
                     body,
                     content='session_text',
                     content_rowid='rowid',
                     -- trigram：中文子串搜索的前提（unicode61 把整段 CJK 当一个 token）
                     tokenize='trigram'
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
                // 升级走增量迁移（v3 起恢复迁移通道）：全部是 ALTER/CREATE 增量语句，
                // 不动既有列，装好的环境无感升级。库比程序新才报错——降级运行会把
                // 新结构当成旧结构用，产生难以排查的数据问题。
                let stored: i64 =
                    conn.query_row("SELECT MAX(version) FROM schema_meta", [], |r| r.get(0))?;
                if stored > SCHEMA_VERSION as i64 {
                    return Err(AppError::new(
                        ErrorKind::Sqlite,
                        format!("库 schema 版本比程序新：库是 v{stored}，程序是 v{SCHEMA_VERSION}"),
                    ));
                }
                if stored < 2 {
                    return Err(AppError::new(
                        ErrorKind::Sqlite,
                        format!("库 schema 版本过旧（v{stored}），请先升级到 v2.0.x 再更新"),
                    ));
                }
                if stored < 3 {
                    Self::migrate_v2_to_v3(conn)?;
                }
                if stored < 4 {
                    Self::migrate_v3_to_v4(conn)?;
                }
            }
            Ok(())
        })
    }

    /// v2 → v3：会话管理扩展（置顶 / 本地命名 / 标签 / 正文全文索引）。
    ///
    /// execute_batch 不是原子的：中途崩溃可能留下「一半迁移」的库。所以 ALTER 用
    /// 逐列检查兜底（重复执行不炸），CREATE 全部带 IF NOT EXISTS，版本号最后才写——
    /// 任何一步失败，下一轮启动都能从 v2 原地重跑。
    fn migrate_v2_to_v3(conn: &Connection) -> Result<()> {
        add_column(conn, "session_marks", "pinned_at", "TEXT")?;
        add_column(conn, "session_marks", "title_override", "TEXT")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS session_tag (
                 source_key TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 tag        TEXT NOT NULL,
                 PRIMARY KEY (source_key, session_id, tag)
             );
             CREATE INDEX IF NOT EXISTS idx_session_tag_tag ON session_tag(tag);
             CREATE TABLE IF NOT EXISTS session_text (
                 source_key TEXT NOT NULL,
                 session_id TEXT NOT NULL,
                 fp         TEXT NOT NULL DEFAULT '',
                 truncated  INTEGER NOT NULL DEFAULT 0,
                 body       TEXT NOT NULL DEFAULT '',
                 UNIQUE (source_key, session_id)
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS session_fts USING fts5(
                 body,
                 content='session_text',
                 content_rowid='rowid',
                 tokenize='trigram'
             );",
        )?;
        conn.execute("UPDATE schema_meta SET version = 3", [])?;
        Ok(())
    }

    /// v3 → v4：签到提交标记表（跨轮次幂等）。
    fn migrate_v3_to_v4(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS checkin_attempts (
                 iid      TEXT NOT NULL,
                 date     TEXT NOT NULL,
                 reserved INTEGER NOT NULL DEFAULT 0,
                 at       INTEGER NOT NULL,
                 PRIMARY KEY (iid, date)
             );",
        )?;
        conn.execute("UPDATE schema_meta SET version = 4", [])?;
        Ok(())
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

/// 表里补一列（已存在就跳过）。迁移要能原地重跑：execute_batch 中途崩溃后，
/// 下一次启动会把已经加过的列再「加」一遍，靠这里的 PRAGMA 检查兜住。
fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let exists = conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|col| col.ok().as_deref() == Some(column));
    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v2 的存量库打开后应无感升到 v3（v3 起恢复迁移通道），签到功能不受影响。
    #[test]
    fn existing_v2_database_migrates_to_current_version() {
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
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        // 迁移产物：marks 新列可写，fts 可查
        db.write(|conn| {
            conn.execute(
                "INSERT INTO session_marks(source_key, session_id, pinned_at)
                 VALUES ('qoder_cn', 's1', '2026-11-15T00:00:00+08:00')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let hits: i64 = db
            .read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM session_fts", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(hits, 0);
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
