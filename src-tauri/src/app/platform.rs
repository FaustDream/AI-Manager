//! 平台能力的落地实现：单实例（契约 12）、静默启动（契约 20）、客户端是否在跑。
//!
//! 这一层刻意很薄——真正的系统调用都在 `process.rs`，这里只做「按平台键分发」，
//! 让契约测试有一个不依赖 Tauri 的入口。

use crate::app::accounts::add::process_names;
use crate::app::error::Result;
use crate::app::process;
use crate::contracts::PlatformApi;
use crate::types::PlatformKey;
use std::path::Path;
use std::sync::Mutex;

pub fn key_name(key: PlatformKey) -> &'static str {
    match key {
        PlatformKey::TraeCn => "trae_cn",
        PlatformKey::Trae => "trae",
        PlatformKey::QoderCn => "qoder",
        PlatformKey::QoderIntl => "qoder_intl",
        PlatformKey::WorkBuddy => "workbuddy",
        PlatformKey::WorkBuddyIntl => "workbuddy_intl",
        PlatformKey::ZCode => "zcode",
    }
}

/// 进程名。Trae 的两个版本各用一个可执行文件，其余复用账号模块的映射。
fn names_of(key: PlatformKey) -> &'static [&'static str] {
    match key {
        PlatformKey::TraeCn => &["Trae CN.exe"],
        PlatformKey::Trae => &["Trae.exe"],
        other => process_names(key_name(other)),
    }
}

/// 持有互斥体句柄的容器。
///
/// **句柄必须一直拿着**：`SingleInstance` 故意没有 Drop，但把 `Option` 丢掉
/// 一样等于放弃互斥体——第二次启动就会「成功」，单实例形同虚设。
pub struct Platform {
    held: Mutex<Vec<process::SingleInstance>>,
}

impl Default for Platform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform {
    pub fn new() -> Self {
        Self {
            held: Mutex::new(Vec::new()),
        }
    }

    /// 进程里已经拿到互斥体了吗（给「第二次启动就退出」的判断用）。
    pub fn holds(&self, name: &str) -> bool {
        let _ = name;
        self.held
            .lock()
            .map(|list| !list.is_empty())
            .unwrap_or(false)
    }
}

impl PlatformApi for Platform {
    fn acquire_single_instance(&self, name: &str) -> Result<bool> {
        match process::acquire_single_instance(name)? {
            Some(handle) => {
                self.held
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(handle);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn spawn_hidden(&self, exe: &str, args: &[&str]) -> Result<()> {
        process::spawn_with_args(Path::new(exe), args, true)
    }

    fn client_running(&self, key: PlatformKey) -> bool {
        let names = names_of(key);
        !names.is_empty() && process::process_running(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cover_every_platform() {
        for key in [
            PlatformKey::TraeCn,
            PlatformKey::Trae,
            PlatformKey::QoderCn,
            PlatformKey::QoderIntl,
            PlatformKey::WorkBuddy,
            PlatformKey::WorkBuddyIntl,
            PlatformKey::ZCode,
        ] {
            assert!(!names_of(key).is_empty(), "{:?} 没有进程名", key);
        }
    }

    #[test]
    fn second_acquire_fails_while_handle_is_held() {
        let platform = Platform::new();
        let name = format!("AIManagerTest{}", std::process::id());
        assert!(platform.acquire_single_instance(&name).unwrap());
        assert!(!platform.acquire_single_instance(&name).unwrap());
    }
}
