//! 测试夹具。每个用例独立临时目录与独立库，绝不碰真实 data/。
//!
//! `Todo*` 一族是各阶段要替换掉的空实现：方法体全是 todo!()。
//!
//! **本文件被每个测试二进制各自 `mod support;` 引一次**，所以「某个二进制里没用到」
//! 是常态而不是死代码——dead_code 在这里只会有噪音。

#![allow(dead_code)]

// `pub use`：`use support::*` 就能拿到 FakeClock 与 Clock trait。
// 只有真去调 `clock.now()` 的二进制用得到 Clock，其余会报 unused_imports。
#[allow(unused_imports)]
pub use ai_manager::app::clock::{Clock, FakeClock};
use ai_manager::app::error::AppError;
use ai_manager::app::paths;
use ai_manager::types::*;
use chrono::{DateTime, Local};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// 进程级串行锁：全局 data_dir override 只有一份，两个并行测试同时用
/// 各自的临时目录必然互相污染（contract_10 / maintenance 都踩过）。
/// TestPaths 持有它到 Drop——同一测试二进制内，任一时刻只有一个测试在用全局目录。
static TEST_DIR_LOCK: Mutex<()> = Mutex::new(());

/// 独立临时目录，Drop 即删。持有串行锁；字段顺序是刻意的——
/// 逆序 Drop 时先删目录再放锁，锁外的下一个测试看到的是自己的新目录。
pub struct TestPaths {
    _lock: MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
}

impl TestPaths {
    pub fn new() -> Self {
        let lock = TEST_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().expect("创建临时目录失败");
        paths::set_data_dir_for_test(dir.path());
        Self { _lock: lock, dir }
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

/// 契约 18 的辅助断言：UTC 输入必须按本地时区渲染。
pub fn render_local(dt: DateTime<Local>) -> String {
    dt.format("%m-%d %H:%M").to_string()
}

pub fn conflict_error(name: &str) -> AppError {
    AppError::new("conflict", format!("{name} 已存在且不是本工具写入"))
}
