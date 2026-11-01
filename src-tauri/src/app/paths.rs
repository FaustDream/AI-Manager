//! 路径的唯一来源。测试里必须能指向临时目录，绝不允许写真实 data/。

use std::path::{Path, PathBuf};
use std::sync::RwLock;

// 可覆盖而不是 OnceLock：OnceLock 会被第一个测试钉死，后续测试在已删除的
// 临时目录里工作，偶发翻车（contract_10 踩过）。串行化由 TestPaths 的
// 进程级锁保证——同一时刻只有一个测试在用全局目录。
static OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// 测试注入点：仅应在 #[cfg(test)] 或 integration test 中调用。
pub fn set_data_dir_for_test(dir: &Path) {
    *OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = Some(dir.to_path_buf());
}

pub fn data_dir() -> PathBuf {
    if let Some(dir) = OVERRIDE.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return dir;
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
        project
            .parent()
            .map(|root| root.join("data"))
            .unwrap_or_else(|| project.join("data"))
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
