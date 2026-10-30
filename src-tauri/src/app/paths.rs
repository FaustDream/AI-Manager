//! 路径的唯一来源。测试里必须能指向临时目录，绝不允许写真实 data/。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// 测试注入点：仅应在 #[cfg(test)] 或 integration test 中调用。
pub fn set_data_dir_for_test(dir: &Path) {
    let _ = OVERRIDE.set(dir.to_path_buf());
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = OVERRIDE.get() {
        return dir.clone();
    }
    // 开发期把数据目录指到项目里的 data/：与 v1 的数据、档案包共用一份，
    // 免得 target/debug/data 与 target/release/data 各有一份互不相认的账号库
    if let Ok(dir) = std::env::var("AIMANAGER_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    // 开发期固定用项目里的 data/：cargo run 的 exe 在 target/debug，
    // 按「exe 同级」会另建一份空库，与 v1 的账号、档案互不相认
    #[cfg(debug_assertions)]
    {
        let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        return project
            .parent()
            .map(|root| root.join("data"))
            .unwrap_or_else(|| project.join("data"));
    }
    // 生产环境：与可执行文件同级的 data/（沿用 v1 的「整个目录拷走即可」约定）
    #[cfg(not(debug_assertions))]
    {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("data")))
            .unwrap_or_else(|| PathBuf::from("data"))
    }
}

pub fn app_db() -> PathBuf {
    data_dir().join("app.db")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// 外部客户端配置目录（技能与 MCP 的目标位置）。契约 13 断言这张表。
pub fn home() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}
