//! 切换登录的编排层：取账号 → 解凭据 → 交给平台实现 → 汇报结果。
//!
//! **这一层不碰客户端文件**。写入面是各家自己的事（Qoder 重封信封、WorkBuddy 换档案、
//! ZCode 整份 JSON），编排层只保证两件事：
//!   1. 读凭据**只读一个账号**（`repo::secret_of`），不批量解；
//!   2. 备份目录统一在 `<data>/backups/switch/`，界面能告诉用户后悔了去哪找。

use super::credential::Credential;
use super::repo;
use crate::app::db::Db;
use crate::app::error::{AppError, Result};
use crate::app::{paths, process, providers};
use serde::Serialize;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct SwitchResult {
    pub message: String,
    /// 写前备份的绝对路径（界面只用来复制/提示，不渲染成正文）
    pub backup: Option<String>,
}

/// 切换登录。会写客户端文件，失败代价是「客户端跳登录页」。
pub fn switch(db: &Db, iid: &str) -> Result<SwitchResult> {
    let identity = repo::identity(db, iid)?
        .ok_or_else(|| AppError::new("invalid", "账号不存在，可能已被删除"))?;
    let secret = repo::secret_of(db, iid)?
        .ok_or_else(|| AppError::new("invalid", "这个账号没有保存登录凭据，无法切换"))?;
    let cred = Credential::parse(&secret)?;

    let backup_dir = paths::data_dir().join("backups").join("switch");
    let outcome = providers::switch(
        iid,
        &identity.account_id,
        &identity.name,
        &cred,
        &backup_dir,
    )?;

    // 各平台 switch_to 都要求客户端先关闭（运行中写入一律拒绝），用户多半是
    // 专为切换才关的——切完自动开回去，对齐 v1 switcher 的重启体验。
    // 没找到 exe 或启动失败都不算切换失败，只落日志。
    let mut message = outcome.message;
    if let Some(exe) = super::add::client_exe(providers::platform_of(iid).unwrap_or_default()) {
        match process::start_detached(&exe) {
            Ok(()) => message.push_str("，客户端已重新启动"),
            Err(e) => crate::app::log::log(
                crate::app::log::Level::Warn,
                "accounts",
                format!("切换后启动客户端失败：{}", e.message),
            ),
        }
    }

    crate::app::log::log(
        crate::app::log::Level::Info,
        "accounts",
        // 日志用昵称，不出现完整 iid（uid 只走「复制 UID」）
        format!("切换登录：{} → {message}", identity.name),
    );
    Ok(SwitchResult {
        message,
        backup: outcome.backup.map(|p| p.display().to_string()),
    })
}

/// 这个账号是不是客户端当前登录的账号。读不到（没装/没登录）一律 false。
pub fn is_current(db: &Db, iid: &str) -> Result<bool> {
    let identity =
        repo::identity(db, iid)?.ok_or_else(|| AppError::new("invalid", "账号不存在"))?;
    let Some(provider) = providers::for_iid(iid) else {
        return Ok(false);
    };
    Ok(provider.current_id().as_deref() == Some(identity.account_id.as_str()))
}

/// 各客户端**当前**登录的账号 iid 列表。界面用它给行标「当前」。
/// 一次遍历六家，比每行各读一次客户端文件便宜得多。
pub fn current_iids() -> Vec<String> {
    let mut out = Vec::new();
    for provider in providers::all() {
        let Some(id) = provider.current_id() else {
            continue;
        };
        // WorkBuddy 的 current_id 已经带了 @intl 后缀，其余平台直接拼
        out.push(format!("{}:{id}", provider.platform()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::accounts::AccountIdentity;

    fn seed(db: &Db, iid: &str) -> AccountIdentity {
        let (platform, account_id) = iid.split_once(':').unwrap();
        let identity = AccountIdentity {
            iid: iid.into(),
            platform: platform.into(),
            platform_label: "Test".into(),
            account_id: account_id.into(),
            name: "测试".into(),
            requires_checkin: true,
            ..Default::default()
        };
        repo::upsert_identity(db, &identity, 1).unwrap();
        identity
    }

    #[test]
    fn unknown_account_is_an_invalid_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let err = switch(&db, "qoder:none").unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Invalid));
    }

    #[test]
    fn account_without_secret_is_refused_before_touching_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "workbuddy:u1");
        let err = switch(&db, "workbuddy:u1").unwrap_err();
        assert_eq!(err.message, "这个账号没有保存登录凭据，无法切换");
    }

    #[test]
    fn corrupt_secret_is_reported_not_panicked() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "qoder:u1");
        repo::set_secret(&db, "qoder:u1", b"not-json", None).unwrap();
        let err = switch(&db, "qoder:u1").unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Invalid));
    }

    #[test]
    fn is_current_is_false_when_client_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "trae:u1");
        // Trae 客户端目录由环境变量决定；无论如何不该 panic，也不该说「是当前」
        assert!(!is_current(&db, "trae:u1").unwrap());
    }
}
