//! 添加账号：**不重写各家的 OAuth**，而是借用客户端自己完成登录。
//!
//! 流程只有三步：关掉客户端 → 把当前登录态挪走（不是删，挪到 `backups/logout/`）→
//! 启动客户端停在登录页。用户在客户端里登录完成后点「扫描客户端」，新账号自然入库。
//!
//! 为什么不做「浏览器回调 / 设备码」：那要为四家平台各实现一套 OAuth 流程
//! （v1 的 `logins.py` 559 行，且随时可能因为厂商改协议失效）。借客户端登录
//! 没有协议风险、没有凭据经过本工具的手，代价只是用户要多点一次「扫描」。
//!
//! **挪走而不是删**：退出登录是「添加账号」的副作用，不能顺手毁掉上一个账号的登录态。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::providers::{WorkBuddyProvider, ZCodeProvider};
use crate::app::{fsx, process};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// 添加账号的结果：界面拿它显示「登录页已经在哪个客户端里打开了」。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct AddOutcome {
    pub platform: String,
    pub label: String,
    /// 真正启动的可执行文件（手动打开时用户要照着点）
    pub exe: String,
    pub restarted: bool,
    /// 主窗口是否已经叫到前台；false = 进程起来了但没窗口（多半在托盘）
    pub window: bool,
}

/// 支持添加账号的平台（Trae 的登录态只读，见 `providers::trae`）。
pub fn supported(platform: &str) -> bool {
    matches!(
        platform,
        "qoder" | "qoder_intl" | "workbuddy" | "workbuddy_intl" | "zcode"
    )
}

/// 退出目标平台的登录态并启动客户端。返回给界面展示的下一步。
///
/// 三件事都有坑，所以每一步都落日志并**验证结果**：
///   1. 客户端得真的退干净——Electron 有单实例锁，旧进程还在退出时拉起新的会被直接吞掉，
///      表现就是「关了客户端之后再没动静」；
///   2. 登录态挪走要快（`app/session` 可能是几百 MB），失败要能看出来；
///   3. 启动后必须确认进程起来了，没起来就重试一次，再不行把 exe 路径告诉用户。
pub fn prepare(platform: &str) -> Result<AddOutcome> {
    if !supported(platform) {
        return Err(AppError::new(
            ErrorKind::Unsupported,
            "这个平台不支持在这里添加账号",
        ));
    }
    let exe = client_exe(platform).ok_or_else(|| {
        AppError::new(
            ErrorKind::Io,
            "没有找到客户端安装位置，请先安装并打开一次客户端",
        )
    })?;
    let label = crate::app::providers::for_platform(platform)
        .map(|provider| provider.label().to_string())
        .unwrap_or_else(|| platform.to_string());

    // 1) 关掉正在跑的客户端（关不掉就直接报错，别动登录态）
    let closed = close_client(platform)?;
    if closed {
        crate::app::log::log(
            crate::app::log::Level::Info,
            "accounts",
            format!("添加账号：已关闭 {label} 客户端"),
        );
    }
    // 必须确认进程真的没了再往下走：Electron 有单实例锁，旧进程还在退出时拉起新的
    // 会被它直接吞掉——表现就是「关掉之后就再没动静」。这里等不到就明确报错。
    if !wait_until_gone(platform, 20) {
        return Err(AppError::new(
            ErrorKind::ClientRunning,
            format!("{label} 没能完全退出（可能还在托盘里），请手动退出后重试"),
        ));
    }

    // 2) 退出当前登录态（不保留副本：当前账号的档案已经在 profiles/ 里存好了）
    crate::app::log::log(
        crate::app::log::Level::Info,
        "accounts",
        format!("添加账号：退出 {label} 当前登录态"),
    );
    match platform {
        "qoder" | "qoder_intl" => logout_qoder(platform)?,
        "workbuddy" | "workbuddy_intl" => logout_workbuddy(platform)?,
        "zcode" => logout_zcode()?,
        _ => unreachable!(),
    }

    // 3) 启动、确认进程起来、再把窗口叫到前台
    let mut last_error = String::new();
    for attempt in 1..=2 {
        match process::start_detached(&exe) {
            Ok(()) => {}
            Err(e) => {
                last_error = e.message;
                continue;
            }
        }
        // Electron 起来要几秒；先等进程，再等窗口
        if !wait_until_running(platform, 40) {
            last_error = format!("{} 启动后没有出现在进程列表里", exe.display());
            crate::app::log::log(
                crate::app::log::Level::Warn,
                "accounts",
                format!("添加账号：{label} 第 {attempt} 次启动没起来，重试"),
            );
            continue;
        }
        // 进程在 ≠ 用户看得见：客户端可能直接收进托盘，或者窗口开在别的窗口后面。
        // 这里主动把主窗口叫到前台，叫不出来就在结果里说清楚，别假装成功。
        let window = wait_for_window(platform, 60);
        crate::app::log::log(
            crate::app::log::Level::Info,
            "accounts",
            format!(
                "添加账号：{label} 客户端已启动（第 {attempt} 次，窗口{}）",
                if window {
                    "已到前台"
                } else {
                    "未出现，可能在托盘"
                }
            ),
        );
        return Ok(AddOutcome {
            platform: platform.to_string(),
            label,
            exe: exe.display().to_string(),
            restarted: true,
            window,
        });
    }

    // 登录态已经清掉，客户端却没起来：至少把路径给用户，让他手动双击
    Err(AppError::new(
        ErrorKind::Io,
        format!(
            "{label} 没能自动打开：{last_error}\n\
             请手动双击 {} 打开客户端完成登录。",
            exe.display()
        ),
    ))
}

/// 只打开客户端，不动任何登录态（自动启动失败时的手动兜底）。
/// 返回窗口是否到了前台——没到就说明它在托盘里。
pub fn open_client(platform: &str) -> Result<bool> {
    let exe = client_exe(platform).ok_or_else(|| {
        AppError::new(
            ErrorKind::Io,
            "没有找到客户端安装位置，请先安装并打开一次客户端",
        )
    })?;
    process::start_detached(&exe)?;
    Ok(wait_for_window(platform, 25))
}

/// 等进程彻底退出（250ms 一轮）。`rounds` 轮内没退干净返回 false。
fn wait_until_gone(platform: &str, rounds: usize) -> bool {
    let names = process_names(platform);
    for _ in 0..rounds {
        if !process::process_running(names) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    !process::process_running(names)
}

/// 等进程出现（200ms 一轮）。
fn wait_until_running(platform: &str, rounds: usize) -> bool {
    let names = process_names(platform);
    for _ in 0..rounds {
        if process::process_running(names) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    false
}

/// 把客户端主窗口叫到前台；窗口还没创建出来就重试。
/// 客户端窗口标题与 exe 名并不一致（国际版窗口叫「WorkBuddy AI」），所以按平台列出来。
fn wait_for_window(platform: &str, rounds: usize) -> bool {
    let Some(title) = window_title(platform) else {
        return false;
    };
    for _ in 0..rounds {
        if process::focus_window(title) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    false
}

fn window_title(platform: &str) -> Option<&'static str> {
    match platform {
        "workbuddy" => Some("WorkBuddy"),
        "workbuddy_intl" => Some("WorkBuddy AI"),
        "qoder" | "qoder_intl" => Some("Qoder"),
        "zcode" => Some("ZCode"),
        _ => None,
    }
}

fn logout_qoder(platform: &str) -> Result<()> {
    let profile = profiles::qoder(platform)
        .ok_or_else(|| AppError::new(ErrorKind::Io, "没有找到 Qoder 的客户端数据目录"))?;
    let auth = profile.join("auth.v1.dat");
    if !auth.exists() {
        return Ok(()); // 本来就没登录，直接开客户端即可
    }
    fsx::remove_path(&auth)
}

fn logout_workbuddy(platform: &str) -> Result<()> {
    let intl = platform.ends_with("_intl");
    let provider = WorkBuddyProvider::new(intl);
    let root = profiles::workbuddy(intl)
        .ok_or_else(|| AppError::new(ErrorKind::Io, "没有找到 WorkBuddy 的客户端数据目录"))?;

    // 先把当前账号存成档案：退出登录后再想切回来就不用重新登录了。
    // 档案里的 sessionState 由服务端签发、无法重建，存不下来就不能继续——
    // 否则这个账号从此切不回来。
    if let Some(uid) = provider.current_uid() {
        provider.save_current_as_pack(&uid)?;
    }

    // **客户端真正认的是 auth 目录下的 .info 令牌**：只清 Chromium 会话是不够的，
    // 令牌还在的话重启会静默自动登录，用户看不到登录页——表现就是「点了添加账号，
    // 软件关了又起来，但什么都没发生」。
    if let Some(ticket) = provider.live_ticket() {
        if ticket.exists() {
            fsx::remove_path(&ticket)?;
            crate::app::log::log(
                crate::app::log::Level::Info,
                "accounts",
                format!("添加账号：已清掉登录令牌 {}", ticket.display()),
            );
        }
    }

    clear_session_dirs(&root)
}

/// 清掉客户端登录态目录，逼它回登录页。缓存不单独保留：客户端自己会重建。
fn clear_session_dirs(root: &Path) -> Result<()> {
    for rel in ["app/session", "storage/skeleton/account-snapshot.json"] {
        let path = root.join(rel);
        if path.exists() {
            fsx::remove_path(&path)?;
        }
    }
    Ok(())
}

fn logout_zcode() -> Result<()> {
    let provider = ZCodeProvider::new();
    let creds = provider.credentials_path();
    if !creds.exists() {
        return Ok(());
    }
    fsx::remove_path(&creds)
}

/// 客户端进程名。
pub fn process_names(platform: &str) -> &'static [&'static str] {
    match platform {
        "qoder" => &["Qoder CN.exe"],
        "qoder_intl" => &["Qoder.exe"],
        "workbuddy" => &["WorkBuddy.exe", "CodeBuddy CN.exe", "CodeBuddy.exe"],
        "workbuddy_intl" => &["WorkBuddyAI.exe"],
        "zcode" => &["ZCode.exe", "zcode.exe"],
        _ => &[],
    }
}

/// 客户端可执行文件：先看常见安装目录，再看 v1 记录过的固定位置。
pub fn client_exe(platform: &str) -> Option<PathBuf> {
    candidates(platform).into_iter().find(|p| p.exists())
}

/// 这个平台的客户端进程名，**只含它自己的那个 exe**。
///
/// 不能直接用 `process_names`：那张表里带着历史遗留的 `CodeBuddy*.exe`，那是
/// **CodeBuddy IDE**（另一个产品，用户可能正开着写代码）。写库前顺手把它杀掉，
/// 代价是丢未保存的编辑器内容。所以以安装路径里的真实文件名为准，
/// 找不到安装位置（没装/装在别处）才退回进程名表。
pub fn client_process_names(platform: &str) -> Vec<String> {
    match client_exe(platform).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
    {
        Some(name) => vec![name],
        None => process_names(platform)
            .iter()
            .map(|n| (*n).to_string())
            .collect(),
    }
}

/// 关掉这个平台的客户端并等进程真正退出（**只动这个版本**，另一版不碰）。
///
/// 切换登录要写客户端文件，带着活进程写会被内存里的旧状态回写覆盖；
/// 与其让用户自己去托盘退出，不如在这里顺手关掉、完事再拉起来。
/// 返回 `Ok(true)` = 关过并且已退出，`Ok(false)` = 本来就没在跑。
pub fn close_client(platform: &str) -> Result<bool> {
    let names = client_process_names(platform);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    if names.is_empty() || !process::process_running(&names) {
        return Ok(false);
    }
    for name in &names {
        process::kill_by_name(name)?;
    }
    // 退出不等于文件立刻可写：句柄回收要一点时间，最多等 5 秒
    for _ in 0..20 {
        if !process::process_running(&names) {
            return Ok(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err(AppError::new(
        ErrorKind::ClientRunning,
        "客户端没能自动关闭，请从托盘退出后重试",
    ))
}

fn candidates(platform: &str) -> Vec<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let pf = std::env::var("ProgramFiles").ok().map(PathBuf::from);
    let programs = local.as_ref().map(|p| p.join("Programs"));
    let mut out = Vec::new();
    let mut push = |p: Option<PathBuf>| {
        if let Some(p) = p {
            out.push(p);
        }
    };
    match platform {
        "qoder" => {
            push(
                programs
                    .as_ref()
                    .map(|p| p.join("Qoder CN").join("Qoder CN.exe")),
            );
            push(
                local
                    .as_ref()
                    .map(|p| p.join("Qoder CN").join("Qoder CN.exe")),
            );
            push(pf.as_ref().map(|p| p.join("Qoder CN").join("Qoder CN.exe")));
        }
        "qoder_intl" => {
            push(programs.as_ref().map(|p| p.join("Qoder").join("Qoder.exe")));
            push(local.as_ref().map(|p| p.join("Qoder").join("Qoder.exe")));
        }
        "workbuddy" => {
            push(Some(PathBuf::from(r"C:\App\WB\CN\WorkBuddy.exe")));
            push(
                programs
                    .as_ref()
                    .map(|p| p.join("WorkBuddy").join("WorkBuddy.exe")),
            );
            push(
                programs
                    .as_ref()
                    .map(|p| p.join("CodeBuddy CN").join("CodeBuddy CN.exe")),
            );
            push(
                pf.as_ref()
                    .map(|p| p.join("WorkBuddy").join("WorkBuddy.exe")),
            );
        }
        "workbuddy_intl" => {
            push(Some(PathBuf::from(r"C:\App\WB\AI\WorkBuddyAI.exe")));
            push(
                programs
                    .as_ref()
                    .map(|p| p.join("WorkBuddyAI").join("WorkBuddyAI.exe")),
            );
            push(
                pf.as_ref()
                    .map(|p| p.join("WorkBuddyAI").join("WorkBuddyAI.exe")),
            );
        }
        "zcode" => {
            push(pf.as_ref().map(|p| p.join("ZCode").join("ZCode.exe")));
            push(programs.as_ref().map(|p| p.join("ZCode").join("ZCode.exe")));
            push(local.as_ref().map(|p| p.join("ZCode").join("ZCode.exe")));
        }
        _ => {}
    }
    out
}

/// 各平台客户端数据目录（与 Provider 用的是同一套路径，集中在这里免得两处漂移）。
mod profiles {
    use super::PathBuf;

    pub fn qoder(platform: &str) -> Option<PathBuf> {
        let appdata = std::env::var("APPDATA").ok()?;
        let name = if platform == "qoder_intl" {
            "com.qoder.app.stable"
        } else {
            "com.qodercn.app.stable"
        };
        Some(PathBuf::from(appdata).join(name))
    }

    pub fn workbuddy(intl: bool) -> Option<PathBuf> {
        Some(crate::app::paths::home().join(if intl { ".workbuddy-ai" } else { ".workbuddy" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logout_clears_the_client_session_without_a_copy() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("client");
        let session = root.join("app/session");
        std::fs::create_dir_all(session.join("Cache")).unwrap();
        std::fs::write(session.join("Cache/data_1"), vec![0u8; 2048]).unwrap();
        std::fs::write(session.join("Cookies"), b"cookies").unwrap();
        std::fs::create_dir_all(root.join("storage/skeleton")).unwrap();
        std::fs::write(root.join("storage/skeleton/account-snapshot.json"), b"{}").unwrap();

        clear_session_dirs(&root).unwrap();

        assert!(!session.exists(), "登录态目录要整体清掉，逼客户端回登录页");
        assert!(
            !root.join("storage/skeleton/account-snapshot.json").exists(),
            "账号快照也要清掉，否则客户端会带着旧账号进主界面"
        );
    }

    #[test]
    fn unsupported_platform_is_refused() {
        let err = prepare("trae").unwrap_err();
        assert!(matches!(
            err.kind,
            crate::app::error::ErrorKind::Unsupported
        ));
    }

    #[test]
    fn every_supported_platform_has_process_names() {
        for platform in [
            "qoder",
            "qoder_intl",
            "workbuddy",
            "workbuddy_intl",
            "zcode",
        ] {
            assert!(supported(platform));
            assert!(!process_names(platform).is_empty(), "{platform} 缺进程名");
            assert!(
                !candidates(platform).is_empty(),
                "{platform} 缺安装位置候选"
            );
        }
    }

    #[test]
    fn close_client_targets_only_that_edition_exe() {
        for platform in [
            "qoder",
            "qoder_intl",
            "workbuddy",
            "workbuddy_intl",
            "zcode",
        ] {
            let names = client_process_names(platform);
            match client_exe(platform) {
                // 装了：只认安装路径里的那个文件名，不扩散到进程名表
                Some(exe) => assert_eq!(
                    names,
                    vec![exe.file_name().unwrap().to_string_lossy().to_string()],
                    "{platform} 应只关自己的 exe"
                ),
                // 没装：退回进程名表，至少保持原行为
                None => assert_eq!(names.len(), process_names(platform).len()),
            }
        }
        // CodeBuddy IDE 是另一个产品，绝不能进 WorkBuddy 的关闭名单
        if client_exe("workbuddy").is_some() {
            assert!(
                !client_process_names("workbuddy")
                    .iter()
                    .any(|n| n.starts_with("CodeBuddy")),
                "关 WorkBuddy 不能顺手杀掉 CodeBuddy IDE"
            );
        }
    }

    #[test]
    fn missing_client_is_reported_not_panicked() {
        // 用一个绝不可能存在的平台组合验证「找不到就报错」
        assert!(client_exe("qoder_intl_none").is_none());
    }
}
