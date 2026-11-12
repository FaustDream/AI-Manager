//! 凭据续期：**先借客户端的手续，续不了才让人重新登录**。
//!
//! 各家平台的 `refresh_token` 续期接口都没有公开协议，自己实现一套等于给自己埋雷
//! （见 `refresh.rs` 顶部的结论）。所以这里做的是**唯一能验证的那条路**：
//! 客户端自己会把 token 续掉，重扫该平台就能拿到新凭据。
//!
//! 判定不靠猜：**凭据真的变了才算续期成功**（token / refresh_token / 到期时间
//! 任一变化）。没变就是没续上——客户端里也是旧凭据，只能重新登录。
//!
//! **边界**：本模块只碰 `data/` 与客户端只读文件，不写任何客户端登录态；
//! 写客户端是 `providers::switch` 的事。

use super::credential::Credential;
use super::{repo, scan};
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::providers;
use crate::app::timeutil::now_ms;

const DAY_MS: i64 = 24 * 3600 * 1000;

/// 续期结论。界面按它决定给什么按钮、说什么话。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenewState {
    /// 客户端里有更新的凭据，已写回本地库
    Renewed,
    /// 凭据仍在有效期内，不需要续
    Fresh,
    /// 客户端里也没有更新的凭据：只能重新登录
    NeedRelogin,
    /// 这个平台不支持续期（只读 / 没有可重扫的登录态）
    Unsupported,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenewOutcome {
    pub state: RenewState,
    /// 给用户看的一句话结论（含下一步该做什么）
    pub message: String,
    /// 续期后的到期时刻；没有到期信息时为 null
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires_at: Option<i64>,
    /// 续期后还剩几天（向下取整，可能为负 = 已过期）
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub days_left: Option<i64>,
}

impl RenewOutcome {
    fn new(state: RenewState, message: String, expires_at: Option<i64>) -> Self {
        Self {
            state,
            message,
            expires_at,
            days_left: expires_at.map(|ms| (ms - now_ms()) / DAY_MS),
        }
    }
}

/// 给一个账号续一次期。会读客户端文件并解密凭据，调用方负责放到阻塞线程里。
pub fn renew(db: &Db, iid: &str) -> Result<RenewOutcome> {
    let bytes = repo::secret_of(db, iid)?.ok_or_else(|| {
        AppError::new(
            ErrorKind::AuthExpired,
            "这个账号没有保存凭据，只能重新登录一次",
        )
    })?;
    let before = Credential::parse(&bytes)?;

    // 平台不认识 / 没有可重扫的客户端登录态：直接说清楚，别让用户白点
    if providers::for_iid(iid).is_none() {
        return Ok(RenewOutcome::new(
            RenewState::Unsupported,
            "这个平台没有可重扫的客户端登录态，续期做不了；请在客户端里登录后点「扫描客户端」"
                .into(),
            before.expires_ms(),
        ));
    }

    let client_key = providers::client_key_of(iid).to_string();
    let report = scan::scan_platform(db, &client_key)?;
    let scan_error = report.errors.first().cloned();

    // 重扫后必须还能读到凭据：客户端被卸载/登出时这里会是 None
    let after_bytes = repo::secret_of(db, iid)?.ok_or_else(|| {
        AppError::new(
            ErrorKind::AuthExpired,
            "重扫后这个账号的凭据不见了，只能重新登录一次",
        )
    })?;
    let after = Credential::parse(&after_bytes)?;

    let before_exp = before.expires_ms();
    let after_exp = after.expires_ms();
    let was_expired = before_exp.map(|ms| ms <= now_ms()).unwrap_or(false);

    // 三个字段任一变化都算「客户端续过」——只看到期时间会漏掉
    // 「token 换了但平台没回到期时间」的平台（Qoder 就是这种）
    let changed = after.token != before.token
        || after.refresh() != before.refresh()
        || match (before_exp, after_exp) {
            (Some(b), Some(a)) => a > b,
            (None, Some(_)) => true,
            _ => false,
        };

    if changed {
        // 新凭据 = 旧的失败标记作废，否则界面会一直挂着「最近一次查询失败」
        if let Err(e) = repo::clear_health_fail(db, iid) {
            crate::app::log::log(
                crate::app::log::Level::Warn,
                "accounts",
                format!("续期后清理失败标记失败：{}", e.message),
            );
        }
        return Ok(RenewOutcome::new(
            RenewState::Renewed,
            "已从客户端取到新凭据并写回，无需重新登录".into(),
            after_exp,
        ));
    }

    if was_expired {
        // 话术按平台分岔：能走应用内浏览器登录的，直接让用户点「重新登录」；
        // 只读平台（Trae）只能去客户端登录，说「重新登录」会让人在界面上找不到按钮
        let mut message = if super::login::supported(&client_key) {
            String::from("客户端里也是同一份旧凭据（平台没有自动续期），只能重新登录一次")
        } else {
            String::from("这个平台只能在客户端里登录：去客户端登录一次，回来点「扫描客户端」")
        };
        if let Some(e) = scan_error {
            message.push_str(&format!("；重扫报错：{e}"));
        }
        return Ok(RenewOutcome::new(
            RenewState::NeedRelogin,
            message,
            after_exp,
        ));
    }

    Ok(RenewOutcome::new(
        RenewState::Fresh,
        "凭据仍在有效期内，不需要续期".into(),
        after_exp,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::accounts::credential::iso_from_ms;
    use crate::app::accounts::AccountIdentity;

    fn setup() -> (tempfile::TempDir, Db) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        (dir, db)
    }

    fn seed(db: &Db, iid: &str) {
        let (platform, account_id) = iid.split_once(':').unwrap();
        repo::upsert_identity(
            db,
            &AccountIdentity {
                iid: iid.into(),
                platform: platform.into(),
                platform_label: "Test".into(),
                account_id: account_id.into(),
                name: "测试账号".into(),
                requires_checkin: true,
                ..Default::default()
            },
            1,
        )
        .unwrap();
    }

    fn cred(token: &str, expires_at: &str) -> Credential {
        Credential {
            token: token.into(),
            refresh_token: "rt".into(),
            expires_at: expires_at.into(),
            ..Default::default()
        }
    }

    fn save(db: &Db, iid: &str, c: &Credential) {
        repo::set_secret(db, iid, &c.to_bytes().unwrap(), c.expires_ms()).unwrap();
    }

    #[test]
    fn missing_credential_is_auth_expired_not_a_panic() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a");
        let err = renew(&db, "qoder:a").unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthExpired);
    }

    #[test]
    fn unknown_platform_is_unsupported() {
        let (_dir, db) = setup();
        seed(&db, "unknown:a");
        save(&db, "unknown:a", &cred("tk", "2099-01-01T00:00:00Z"));
        let out = renew(&db, "unknown:a").unwrap();
        assert_eq!(out.state, RenewState::Unsupported);
    }

    /// 客户端目录不存在的机器上重扫扫不到任何东西：凭据没变 → 未过期就是 Fresh
    #[test]
    fn unchanged_valid_credential_is_fresh() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a");
        let c = cred("tk", &iso_from_ms(now_ms() + 10 * DAY_MS));
        save(&db, "qoder:a", &c);

        let out = renew(&db, "qoder:a").unwrap();

        assert_eq!(out.state, RenewState::Fresh, "{}", out.message);
        assert!(out.days_left.unwrap() >= 9);
    }

    /// 已过期且重扫拿不到新凭据 → 必须明确要求重新登录，而不是含糊的「失败」
    #[test]
    fn expired_without_new_credential_needs_relogin() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a");
        let c = cred("tk", "2000-01-01T00:00:00Z");
        save(&db, "qoder:a", &c);

        let out = renew(&db, "qoder:a").unwrap();

        assert_eq!(out.state, RenewState::NeedRelogin);
        assert!(out.message.contains("重新登录"));
    }

    /// Trae 没有应用内登录入口，话术必须改成「去客户端登录」，否则用户找不到按钮
    #[test]
    fn read_only_platform_points_at_the_client_instead_of_relogin() {
        let (_dir, db) = setup();
        seed(&db, "trae:a");
        let c = cred("tk", "2000-01-01T00:00:00Z");
        save(&db, "trae:a", &c);

        let out = renew(&db, "trae:a").unwrap();

        assert_eq!(out.state, RenewState::NeedRelogin);
        assert!(out.message.contains("客户端"), "{}", out.message);
        assert!(!out.message.contains("重新登录"), "{}", out.message);
    }

    #[test]
    fn days_left_is_negative_after_expiry() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a");
        let c = cred("tk", "2000-01-01T00:00:00Z");
        save(&db, "qoder:a", &c);
        let out = renew(&db, "qoder:a").unwrap();
        assert!(out.days_left.unwrap() < 0);
    }
}
