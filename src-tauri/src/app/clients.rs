//! 各家客户端的数据目录，以及**读写分家的库连接**（契约 9）。
//!
//! 读客户端库一律 `SQLITE_OPEN_READ_ONLY`：客户端可能正在写，我们只是路过看一眼。
//! 写之前必须先 `wal_checkpoint(TRUNCATE)`，否则 WAL 里那截还没落盘的数据会被
//! 我们的写入挤坏——契约 9b 就是这条。
//!
//! 路径全部经 `Roots`，测试可以指向临时目录，绝不允许碰真实客户端。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::contracts::ClientDb;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// 三个基准目录。生产取环境变量，测试用 `at()` 指到临时目录。
#[derive(Debug, Clone)]
pub struct Roots {
    pub home: PathBuf,
    pub appdata: PathBuf,
    pub localappdata: PathBuf,
}

impl Roots {
    pub fn system() -> Self {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Roaming"));
        let localappdata = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Local"));
        Self {
            home,
            appdata,
            localappdata,
        }
    }

    pub fn at(
        home: impl Into<PathBuf>,
        appdata: impl Into<PathBuf>,
        local: impl Into<PathBuf>,
    ) -> Self {
        Self {
            home: home.into(),
            appdata: appdata.into(),
            localappdata: local.into(),
        }
    }

    /// `~/.workbuddy`（国内版）或 `~/.workbuddy-ai`（国际版）
    pub fn workbuddy(&self, intl: bool) -> PathBuf {
        self.home
            .join(if intl { ".workbuddy-ai" } else { ".workbuddy" })
    }

    /// `%APPDATA%\com.qodercn.app.stable` / `com.qoder.app.stable`
    pub fn qoder(&self, intl: bool) -> PathBuf {
        self.appdata.join(if intl {
            "com.qoder.app.stable"
        } else {
            "com.qodercn.app.stable"
        })
    }

    pub fn zcode(&self) -> PathBuf {
        self.home.join(".zcode")
    }

    /// CodeBuddy 的会话目录：`%LOCALAPPDATA%\CodeBuddyExtension\Data`
    pub fn codebuddy(&self) -> PathBuf {
        self.localappdata.join("CodeBuddyExtension").join("Data")
    }

    /// Trae 只有云端会话，本地目录用来判断「装没装」
    pub fn trae(&self, cn: bool) -> PathBuf {
        self.appdata.join(if cn { "Trae CN" } else { "Trae" })
    }
}

/// 写客户端库前清 WAL：客户端读不到未 checkpoint 的改动。
pub fn checkpoint(conn: &Connection) -> Result<()> {
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(()),
            other => Err(AppError::new(
                ErrorKind::Sqlite,
                format!("wal_checkpoint 失败：{other}"),
            )),
        })
}

/// 一个客户端库连接。读用只读 flag，写先 checkpoint。
pub struct ClientConn {
    conn: Mutex<Option<Connection>>,
    path: Mutex<Option<PathBuf>>,
    readonly: AtomicBool,
}

impl Default for ClientConn {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientConn {
    pub fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            path: Mutex::new(None),
            readonly: AtomicBool::new(false),
        }
    }

    pub fn is_readonly(&self) -> bool {
        self.readonly.load(Ordering::Relaxed)
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.path.lock().ok().and_then(|p| p.clone())
    }

    /// `write=false` → `SQLITE_OPEN_READ_ONLY`；文件不存在直接报错，不替它建库。
    pub fn open(&self, path: &Path, write: bool) -> Result<()> {
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        if !path.exists() {
            return Err(AppError::new(
                ErrorKind::Io,
                format!("库不存在：{}", path.display()),
            ));
        }
        let conn = Connection::open_with_flags(path, flags).map_err(|e| {
            AppError::new(
                ErrorKind::Sqlite,
                format!("打开 {} 失败：{e}", path.display()),
            )
        })?;
        *self.conn.lock().unwrap_or_else(|e| e.into_inner()) = Some(conn);
        *self.path.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.to_path_buf());
        self.readonly.store(!write, Ordering::Relaxed);
        Ok(())
    }

    /// 契约 9b：写路径在事务之前调用。`TRUNCATE` 会把 WAL 清空并重置，
    /// 比 `PASSIVE` 多一步截断，写完之后客户端重新打开不会读到半截状态。
    pub fn checkpoint(&self) -> Result<()> {
        self.with(crate::app::clients::checkpoint)
    }

    /// 只读连接上执行写语句，供契约 9a 的断言使用。
    pub fn execute(&self, sql: &str) -> Result<usize> {
        self.with(|conn| {
            conn.execute(sql, [])
                .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("执行失败：{e}")))
        })
    }

    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| AppError::new(ErrorKind::Sqlite, "连接已失效"))?;
        let conn = guard
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "连接未打开"))?;
        f(conn)
    }

    pub fn close(&self) {
        *self.conn.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.path.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.readonly.store(false, Ordering::Relaxed);
    }
}

impl ClientDb for ClientConn {
    fn open(&self, path: &Path, write: bool) -> Result<()> {
        ClientConn::open(self, path, write)
    }

    fn checkpoint(&self) -> Result<()> {
        ClientConn::checkpoint(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(dir: &Path) -> PathBuf {
        let path = dir.join("client.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE sessions(id TEXT PRIMARY KEY, title TEXT);",
        )
        .unwrap();
        path
    }

    #[test]
    fn readonly_flag_is_set_and_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_db(dir.path());
        let db = ClientConn::new();
        db.open(&path, false).unwrap();
        assert!(db.is_readonly());
        // 只读连接上写必然失败——这条比「flag 等于多少」更接近真实风险
        assert!(db.execute("INSERT INTO sessions VALUES ('a','b')").is_err());
    }

    #[test]
    fn checkpoint_empties_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = temp_db(dir.path());
        let db = ClientConn::new();
        db.open(&path, true).unwrap();
        db.execute("INSERT INTO sessions VALUES ('a','b')").unwrap();
        db.checkpoint().unwrap();
        let wal = PathBuf::from(format!("{}-wal", path.display()));
        assert_eq!(
            std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0),
            0,
            "checkpoint 之后 WAL 应该是空的"
        );
    }
}
