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

use crate::app::error::{AppError, Result};
use crate::app::providers::{WorkBuddyProvider, ZCodeProvider};
use crate::app::{fsx, paths, process};
use std::path::{Path, PathBuf};

/// 支持添加账号的平台（Trae 的登录态只读，见 `providers::trae`）。
pub fn supported(platform: &str) -> bool {
    matches!(platform, "qoder" | "qoder_intl" | "workbuddy" | "workbuddy_intl" | "zcode")
}

/// 退出目标平台的登录态并启动客户端。返回给用户看的下一步提示。
pub fn prepare(platform: &str) -> Result<String> {
    if !supported(platform) {
        return Err(AppError::new("unsupported", "这个平台不支持在这里添加账号"));
    }
    let exe = client_exe(platform)
        .ok_or_else(|| AppError::new("io", "没有找到客户端安装位置，请先安装并打开一次客户端"))?;
    let names = process_names(platform);
    if process::process_running(names) {
        return Err(AppError::new(
            "client_running",
            "请先关闭客户端再添加账号（关闭窗口不够，需要从托盘退出）",
        ));
    }

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let keep = paths::data_dir().join("backups").join("logout").join(format!("{platform}-{stamp}"));

    match platform {
        "qoder" | "qoder_intl" => logout_qoder(platform, &keep)?,
        "workbuddy" | "workbuddy_intl" => logout_workbuddy(platform, &keep)?,
        "zcode" => logout_zcode(&keep)?,
        _ => unreachable!(),
    }

    process::start_detached(&exe)?;
    Ok(format!(
        "已退出登录并启动客户端。请在客户端里登录新账号，登录完成后回到这里点「扫描客户端」。\n\
         原来的登录态已备份到 {}",
        keep.display()
    ))
}

fn logout_qoder(platform: &str, keep: &Path) -> Result<()> {
    let profile = profiles::qoder(platform)
        .ok_or_else(|| AppError::new("io", "没有找到 Qoder 的客户端数据目录"))?;
    let auth = profile.join("auth.v1.dat");
    if !auth.exists() {
        return Ok(()); // 本来就没登录，直接开客户端即可
    }
    fsx::backup_to(&auth, keep, platform)?;
    fsx::remove_path(&auth)
}

fn logout_workbuddy(platform: &str, keep: &Path) -> Result<()> {
    let intl = platform.ends_with("_intl");
    let provider = WorkBuddyProvider::new(intl);
    let root = profiles::workbuddy(intl)
        .ok_or_else(|| AppError::new("io", "没有找到 WorkBuddy 的客户端数据目录"))?;

    // 先把当前账号存成档案：退出登录后再想切回来就不用重新登录了
    if let Some(uid) = provider.current_uid() {
        let _ = provider.save_current_as_pack(&uid);
    }
    for rel in ["app/session", "storage/skeleton/account-snapshot.json"] {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let target = keep.join(rel);
        if path.is_dir() {
            fsx::copy_dir_merge(&path, &target)?;
            fsx::remove_path(&path)?;
        } else {
            std::fs::create_dir_all(target.parent().unwrap_or(keep))?;
            std::fs::copy(&path, &target)?;
            fsx::remove_path(&path)?;
        }
    }
    Ok(())
}

fn logout_zcode(keep: &Path) -> Result<()> {
    let provider = ZCodeProvider::new();
    let creds = provider.credentials_path();
    if !creds.exists() {
        return Ok(());
    }
    fsx::backup_to(&creds, keep, "zcode")?;
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
            push(programs.as_ref().map(|p| p.join("Qoder CN").join("Qoder CN.exe")));
            push(local.as_ref().map(|p| p.join("Qoder CN").join("Qoder CN.exe")));
            push(pf.as_ref().map(|p| p.join("Qoder CN").join("Qoder CN.exe")));
        }
        "qoder_intl" => {
            push(programs.as_ref().map(|p| p.join("Qoder").join("Qoder.exe")));
            push(local.as_ref().map(|p| p.join("Qoder").join("Qoder.exe")));
        }
        "workbuddy" => {
            push(Some(PathBuf::from(r"C:\App\WB\CN\WorkBuddy.exe")));
            push(programs.as_ref().map(|p| p.join("WorkBuddy").join("WorkBuddy.exe")));
            push(programs.as_ref().map(|p| p.join("CodeBuddy CN").join("CodeBuddy CN.exe")));
            push(pf.as_ref().map(|p| p.join("WorkBuddy").join("WorkBuddy.exe")));
        }
        "workbuddy_intl" => {
            push(Some(PathBuf::from(r"C:\App\WB\AI\WorkBuddyAI.exe")));
            push(programs.as_ref().map(|p| p.join("WorkBuddyAI").join("WorkBuddyAI.exe")));
            push(pf.as_ref().map(|p| p.join("WorkBuddyAI").join("WorkBuddyAI.exe")));
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
        let name = if platform == "qoder_intl" { "com.qoder.app.stable" } else { "com.qodercn.app.stable" };
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
    fn unsupported_platform_is_refused() {
        let err = prepare("trae").unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Unsupported));
    }

    #[test]
    fn every_supported_platform_has_process_names() {
        for platform in ["qoder", "qoder_intl", "workbuddy", "workbuddy_intl", "zcode"] {
            assert!(supported(platform));
            assert!(!process_names(platform).is_empty(), "{platform} 缺进程名");
            assert!(!candidates(platform).is_empty(), "{platform} 缺安装位置候选");
        }
    }

    #[test]
    fn missing_client_is_reported_not_panicked() {
        // 用一个绝不可能存在的平台组合验证「找不到就报错」
        assert!(client_exe("qoder_intl_none").is_none());
    }
}
