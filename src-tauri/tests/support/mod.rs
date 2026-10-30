//! 测试夹具。每个用例独立临时目录与独立库，绝不碰真实 data/。
//!
//! `Todo*` 一族是各阶段要替换掉的空实现：方法体全是 todo!()，
//! 因此本阶段 22 个契约测试**必然全红**——这是 P0 的预期状态。

// pub use：让 `use support::*` 带上 Clock trait，否则测试里调不到 clock.now()
pub use ai_manager::app::clock::{Clock, FakeClock};
use ai_manager::app::error::{AppError, Result};
use ai_manager::app::paths;
use ai_manager::contracts::*;
use ai_manager::types::*;
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};

/// 独立临时目录，Drop 即删。
pub struct TestPaths {
    dir: tempfile::TempDir,
}

impl TestPaths {
    pub fn new() -> Self {
        let dir = tempfile::TempDir::new().expect("创建临时目录失败");
        paths::set_data_dir_for_test(dir.path());
        Self { dir }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// 写入一个文件并返回其路径。
    pub fn write(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).ok();
        std::fs::write(&path, content).expect("写入临时文件失败");
        path
    }

    pub fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.join(name)).expect("读取临时文件失败")
    }
}

/// 固定时刻的时钟，避免任何用例依赖真实时间。
pub fn clock_at(rfc3339: &str) -> FakeClock {
    FakeClock::at(rfc3339)
}

pub fn local_dt(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
    ai_manager::app::clock::local(y, m, d, h, min)
}

pub fn sample_account(iid: &str, _live: bool) -> Account {
    Account {
        iid: iid.to_string(),
        platform: PlatformKey::QoderCn,
        platform_label: "Qoder CN".into(),
        account_id: iid.split(':').nth(1).unwrap_or(iid).to_string(),
        name: "测试账号".into(),
        expires_at: None,
        source: "本地凭据".into(),
        requires_checkin: true,
        meta_keys: vec!["region".into()],
    }
}

macro_rules! todo_impl {
    ($trait:ident for $ty:ident { $(fn $m:ident($($arg:ident: $t:ty),*) $(-> $ret:ty)?;)* }) => {
        pub struct $ty;
        impl $trait for $ty {
            $(fn $m(&self, $(_: $t),*) $(-> $ret)? { todo!("P1~P6 实现") })*
        }
    };
}

todo_impl!(CheckinStore for TodoCheckin {
    fn resolve_checked(iid: &str, remote: Option<bool>, clock: &dyn Clock) -> bool;
    fn marked_today(iid: &str, clock: &dyn Clock) -> bool;
    fn mark_today(iid: &str, clock: &dyn Clock) -> Result<()>;
    fn next_at(iid: &str) -> Option<DateTime<Local>>;
    fn note_next_at(iid: &str, at: DateTime<Local>) -> Result<()>;
    fn next_checkin_at(cooldown_hours: u32, last: DateTime<Local>, clock: &dyn Clock) -> Option<DateTime<Local>>;
    fn note_balance(iid: &str, balance: f64, clock: &dyn Clock) -> Result<bool>;
    fn balance_on(iid: &str, day: &str) -> Option<f64>;
});

todo_impl!(Ledger for TodoLedger {
    fn add_if_needed(live_credit: bool, iid: &str, granted: f64, clock: &dyn Clock) -> Result<bool>;
    fn count(iid: &str) -> usize;
});

todo_impl!(SessionStore for TodoSessions {
    fn page(cursor: Option<&str>, limit: usize, state: Option<&str>) -> Result<Page<SessionRow>>;
    fn restore_account(edition: &str, uid: &str) -> Result<RestoreReport>;
});

todo_impl!(HubStore for TodoHub {
    fn target_dir(key: TargetKey) -> PathBuf;
    fn to_platform(entry: &McpEntry, key: TargetKey) -> serde_json::Value;
    fn from_platform(raw: &serde_json::Value, key: TargetKey) -> Result<McpEntry>;
    fn backup_before_write(key: TargetKey) -> Result<PathBuf>;
    fn check_conflict(name: &str, key: TargetKey) -> Result<()>;
});

todo_impl!(PlatformApi for TodoPlatform {
    fn acquire_single_instance(name: &str) -> Result<bool>;
    fn spawn_hidden(exe: &str, args: &[&str]) -> Result<()>;
    fn client_running(key: PlatformKey) -> bool;
});

/// 契约 18 的辅助断言：UTC 输入必须按本地时区渲染。
pub fn render_local(dt: DateTime<Local>) -> String {
    dt.format("%m-%d %H:%M").to_string()
}

pub fn conflict_error(name: &str) -> AppError {
    AppError::new("conflict", format!("{name} 已存在且不是本工具写入"))
}
