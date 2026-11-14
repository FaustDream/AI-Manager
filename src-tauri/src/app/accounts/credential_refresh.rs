//! 凭据刷新：**每次都重扫客户端，把最新凭据与有效期写回本地库**。
//!
//! 与「续期」的差别是语义：续期只在快到期时才动，这里的结论只有两种——
//! 「刷新到了更新的凭据」和「客户端里就是最新的一份」。**不再因为「还在有效期内」
//! 就跳过**，因为平台侧的有效期可能已经被客户端续过，而本地库还停在扫描那一刻。
//!
//! 各家平台的 `refresh_token` 续期接口都没有公开协议，自己实现一套等于给自己埋雷
//! （见 `refresh.rs` 顶部的结论）。所以这里走**唯一能验证的那条路**：客户端自己会
//! 把 token 续掉，重扫该平台就能拿到新凭据。
//!
//! 判定不靠猜：**凭据真的变了才算刷新到新东西**（token / refresh_token / 到期时间
//! 任一变化）。没变就是「已是最新」——此时有效期保持原值，不伪造更晚的时间。
//!
//! **边界**：本模块只碰 `data/` 与客户端只读文件，不写任何客户端登录态；
//! 写客户端是 `providers::switch` 的事。

use super::credential::Credential;
use super::{repo, scan};
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::providers;
use crate::app::timeutil::now_ms;
use std::collections::BTreeMap;

const DAY_MS: i64 = 24 * 3600 * 1000;

/// 刷新结论。界面按它决定说什么话、给什么下一步。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshCredentialState {
    /// 客户端里有更新的凭据，已写回，有效期已跟着更新
    Refreshed,
    /// 客户端里就是最新的一份，没有可更新的内容（有效期保持原值）
    UpToDate,
    /// 刷新后凭据仍然过期：客户端里也是旧的，只能重新登录
    NeedRelogin,
    /// 这个平台没有可重扫的登录态
    Unsupported,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct RefreshCredentialOutcome {
    pub state: RefreshCredentialState,
    /// 给用户看的一句话结论（含下一步该做什么）
    pub message: String,
    /// 刷新后的到期时刻；没有到期信息时为 null
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires_at: Option<i64>,
    /// 刷新后还剩几天（向下取整，可能为负 = 已过期）
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub days_left: Option<i64>,
}

impl RefreshCredentialOutcome {
    fn new(state: RefreshCredentialState, message: String, expires_at: Option<i64>) -> Self {
        Self {
            state,
            message,
            expires_at,
            days_left: expires_at.map(|ms| (ms - now_ms()) / DAY_MS),
        }
    }
}

/// 批量刷新的汇总。`errors` 只收真正出错的行（凭据解不开、写盘失败），
/// 「平台不支持」「需要重新登录」是**结论**，各有自己的计数。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct RefreshCredentialsReport {
    /// 本次处理的账号数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
    /// 刷新到新凭据（有效期已更新）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub refreshed: usize,
    /// 已是最新，没有可更新的内容
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub up_to_date: usize,
    /// 刷新后仍然过期，需要重新登录
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub need_relogin: usize,
    /// 平台没有可重扫的登录态
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub unsupported: usize,
    /// 读凭据 / 解析失败
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub failed: usize,
    pub errors: Vec<String>,
    /// 整次刷新的耗时
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ms: u64,
}

/// 刷新前的凭据指纹：只留比对用的三个字段。
struct Snapshot {
    token: String,
    refresh: String,
    expires: Option<i64>,
}

fn snapshot(db: &Db, iid: &str) -> Result<Snapshot> {
    let bytes = repo::secret_of(db, iid)?.ok_or_else(|| {
        AppError::new(
            ErrorKind::AuthExpired,
            "这个账号没有保存凭据，只能重新登录一次",
        )
    })?;
    let cred = Credential::parse(&bytes)?;
    // 先取 refresh / expires，再把 token move 出去：`refresh()` 借的是 cred
    let refresh = cred.refresh().to_string();
    let expires = cred.expires_ms();
    Ok(Snapshot {
        token: cred.token,
        refresh,
        expires,
    })
}

/// 用刷新后的凭据和刷新前的指纹比对，给出结论。
fn judge(
    db: &Db,
    iid: &str,
    before: &Snapshot,
    scan_error: Option<&str>,
) -> Result<RefreshCredentialOutcome> {
    let bytes = repo::secret_of(db, iid)?.ok_or_else(|| {
        AppError::new(
            ErrorKind::AuthExpired,
            "刷新后这个账号的凭据不见了，只能重新登录一次",
        )
    })?;
    let after = Credential::parse(&bytes)?;
    let after_exp = after.expires_ms();

    // 三个字段任一变化都算「客户端续过」——只看到期时间会漏掉
    // 「token 换了但平台没回到期时间」的平台（Qoder 就是这种）
    let changed = after.token != before.token
        || after.refresh() != before.refresh
        || match (before.expires, after_exp) {
            (Some(b), Some(a)) => a > b,
            (None, Some(_)) => true,
            _ => false,
        };

    // 刷新完还是过期的，说明客户端里也没有新凭据：这时候「已是最新」没有意义，
    // 必须明确告诉用户要重新登录
    if after_exp.map(|ms| ms <= now_ms()).unwrap_or(false) {
        let mut message = if super::login::supported(providers::client_key_of(iid)) {
            String::from("刷新后凭据仍是过期的：客户端里也是同一份旧凭据，只能重新登录一次")
        } else {
            String::from("刷新后凭据仍是过期的：这个平台只能在客户端里登录，登录后回来再刷新")
        };
        if let Some(e) = scan_error {
            message.push_str(&format!("；重扫报错：{e}"));
        }
        // 重扫之后这条还是过期的，而客户端里登录的是另一个有效账号：
        // 那是重登换发了 userId，旧行成了孤儿——「去重新登录」的指引是错的，把实情说明白
        if let Some(hint) = orphan_login_hint(db, iid) {
            message.push_str(&format!("；{hint}"));
        }
        return Ok(RefreshCredentialOutcome::new(
            RefreshCredentialState::NeedRelogin,
            message,
            after_exp,
        ));
    }

    if changed {
        // 新凭据 = 旧的失败标记作废，否则界面会一直挂着「最近一次查询失败」
        if let Err(e) = repo::clear_health_fail(db, iid) {
            crate::app::log::log(
                crate::app::log::Level::Warn,
                "accounts",
                format!("刷新凭据后清理失败标记失败：{}", e.message),
            );
        }
        return Ok(RefreshCredentialOutcome::new(
            RefreshCredentialState::Refreshed,
            "已从客户端取到最新凭据并写回，有效期已更新".into(),
            after_exp,
        ));
    }

    Ok(RefreshCredentialOutcome::new(
        RefreshCredentialState::UpToDate,
        "客户端里就是最新的一份，有效期保持原值".into(),
        after_exp,
    ))
}

/// 同平台有另一条凭据仍有效的账号 → 客户端当前登录的不是这一条。
///
/// 重新登录会换发 userId（新身份新行），旧行就成了永远续不上的孤儿；
/// 这时「去客户端重新登录」的指引是错的——客户端里登录的明明是另一个账号。
/// 只在重扫**之后**判断（`scan_platform` 刚落完库），DB 就是客户端的当前实况。
fn orphan_login_hint(db: &Db, iid: &str) -> Option<String> {
    let (platform, _) = iid.split_once(':')?;
    for row in repo::list(db).ok()? {
        if row.platform != platform || row.iid == iid || !row.has_secret {
            continue;
        }
        let Ok(Some(bytes)) = repo::secret_of(db, &row.iid) else {
            continue;
        };
        let Ok(cred) = Credential::parse(&bytes) else {
            continue;
        };
        if cred.expires_ms().is_some_and(|ms| ms > now_ms()) {
            return Some(format!(
                "客户端当前登录的是「{}」，凭据有效；这条是旧登录态，确认不再使用可在账号中心删除",
                row.display_name()
            ));
        }
    }
    None
}

/// 刷新一个账号的凭据。会读客户端文件并解密凭据，调用方负责放到阻塞线程里。
pub fn refresh_credential(db: &Db, iid: &str) -> Result<RefreshCredentialOutcome> {
    let before = snapshot(db, iid)?;
    if providers::for_iid(iid).is_none() {
        return Ok(RefreshCredentialOutcome::new(
            RefreshCredentialState::Unsupported,
            "这个平台没有可重扫的客户端登录态，刷新不了；请在客户端里登录后点「刷新账号状态」"
                .into(),
            before.expires,
        ));
    }
    let client_key = providers::client_key_of(iid).to_string();
    let report = scan::scan_platform(db, &client_key)?;
    judge(db, iid, &before, report.errors.first().map(String::as_str))
}

/// 批量刷新。**按平台分组，每个平台只重扫一次**——逐个账号扫会把同一家客户端
/// 读 N 遍，几十个账号就是几十次磁盘解密。
pub fn refresh_credentials(db: &Db, iids: &[String]) -> Result<RefreshCredentialsReport> {
    let started = std::time::Instant::now();
    let mut report = RefreshCredentialsReport {
        total: iids.len(),
        ..Default::default()
    };

    // 1) 刷新前快照，同时按客户端平台键分组
    let mut groups: BTreeMap<String, Vec<(String, Snapshot)>> = BTreeMap::new();
    for iid in iids {
        if providers::for_iid(iid).is_none() {
            report.unsupported += 1;
            continue;
        }
        match snapshot(db, iid) {
            Ok(before) => {
                groups
                    .entry(providers::client_key_of(iid).to_string())
                    .or_default()
                    .push((iid.clone(), before));
            }
            Err(e) => {
                report.failed += 1;
                report.errors.push(format!("{iid}：{}", e.message));
            }
        }
    }

    // 2) 每个平台扫一次，再逐个比对
    for (client_key, items) in groups {
        let scan_error = match scan::scan_platform(db, &client_key) {
            Ok(r) => r.errors.first().cloned(),
            Err(e) => Some(e.message),
        };
        for (iid, before) in items {
            match judge(db, &iid, &before, scan_error.as_deref()) {
                Ok(out) => match out.state {
                    RefreshCredentialState::Refreshed => report.refreshed += 1,
                    RefreshCredentialState::UpToDate => report.up_to_date += 1,
                    RefreshCredentialState::NeedRelogin => report.need_relogin += 1,
                    RefreshCredentialState::Unsupported => report.unsupported += 1,
                },
                Err(e) => {
                    report.failed += 1;
                    report.errors.push(format!("{iid}：{}", e.message));
                }
            }
        }
    }

    report.ms = started.elapsed().as_millis() as u64;
    Ok(report)
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
        let err = refresh_credential(&db, "qoder:a").unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthExpired);
    }

    #[test]
    fn unknown_platform_is_unsupported() {
        let (_dir, db) = setup();
        seed(&db, "unknown:a");
        save(&db, "unknown:a", &cred("tk", "2099-01-01T00:00:00Z"));
        let out = refresh_credential(&db, "unknown:a").unwrap();
        assert_eq!(out.state, RefreshCredentialState::Unsupported);
    }

    /// 客户端目录不存在的机器上重扫扫不到新东西：**仍然走一遍刷新**，
    /// 结论是「已是最新」而不是「还没到期所以不用刷」。
    #[test]
    fn unchanged_valid_credential_is_up_to_date_not_skipped() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a");
        let c = cred("tk", &iso_from_ms(now_ms() + 10 * DAY_MS));
        save(&db, "qoder:a", &c);

        let out = refresh_credential(&db, "qoder:a").unwrap();

        assert_eq!(
            out.state,
            RefreshCredentialState::UpToDate,
            "{}",
            out.message
        );
        assert!(out.days_left.unwrap() >= 9);
    }

    /// 已过期且刷新拿不到新凭据 → 必须明确要求重新登录，而不是含糊的「失败」
    #[test]
    fn expired_after_refresh_needs_relogin() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a");
        let c = cred("tk", "2000-01-01T00:00:00Z");
        save(&db, "qoder:a", &c);

        let out = refresh_credential(&db, "qoder:a").unwrap();

        assert_eq!(out.state, RefreshCredentialState::NeedRelogin);
        assert!(out.message.contains("重新登录"));
    }

    /// Trae 没有应用内登录入口，话术必须改成「去客户端登录」，否则用户找不到按钮
    #[test]
    fn read_only_platform_points_at_the_client_instead_of_relogin() {
        let (_dir, db) = setup();
        seed(&db, "trae:a");
        let c = cred("tk", "2000-01-01T00:00:00Z");
        save(&db, "trae:a", &c);

        let out = refresh_credential(&db, "trae:a").unwrap();

        assert_eq!(out.state, RefreshCredentialState::NeedRelogin);
        assert!(out.message.contains("客户端"), "{}", out.message);
        assert!(!out.message.contains("重新登录"), "{}", out.message);
    }

    /// 重登换发了 userId 后，客户端里登录的是另一个有效账号、旧行永远续不上：
    /// 这时不能只说「去客户端登录」，要直接点明这条是旧登录态（孤儿行）。
    /// 直调 judge 而非 refresh_credential——后者会真扫本机客户端目录，结果随环境漂。
    #[test]
    fn orphan_hint_names_the_currently_logged_in_account() {
        let (_dir, db) = setup();
        seed(&db, "trae:old");
        save(&db, "trae:old", &cred("old-tk", "2000-01-01T00:00:00Z"));
        seed(&db, "trae:new");
        save(
            &db,
            "trae:new",
            &cred("new-tk", &iso_from_ms(now_ms() + 10 * DAY_MS)),
        );

        let before = snapshot(&db, "trae:old").unwrap();
        let out = judge(&db, "trae:old", &before, None).unwrap();

        assert_eq!(out.state, RefreshCredentialState::NeedRelogin);
        assert!(
            out.message.contains("客户端当前登录的是"),
            "{}",
            out.message
        );
        assert!(out.message.contains("测试账号"), "{}", out.message);
    }

    /// 同平台没有别的有效账号时不能乱提示：客户端里可能真就是这份旧凭据
    #[test]
    fn no_orphan_hint_when_no_other_valid_account() {
        let (_dir, db) = setup();
        seed(&db, "trae:old");
        save(&db, "trae:old", &cred("old-tk", "2000-01-01T00:00:00Z"));
        seed(&db, "trae:other");
        save(&db, "trae:other", &cred("tk", "2000-01-01T00:00:00Z"));

        let before = snapshot(&db, "trae:old").unwrap();
        let out = judge(&db, "trae:old", &before, None).unwrap();

        assert_eq!(out.state, RefreshCredentialState::NeedRelogin);
        assert!(
            !out.message.contains("客户端当前登录的是"),
            "{}",
            out.message
        );
    }

    #[test]
    fn days_left_is_negative_after_expiry() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a");
        let c = cred("tk", "2000-01-01T00:00:00Z");
        save(&db, "qoder:a", &c);
        let out = refresh_credential(&db, "qoder:a").unwrap();
        assert!(out.days_left.unwrap() < 0);
    }

    /// 批量：不支持的平台单独计数，不拖累其它账号
    #[test]
    fn batch_counts_unsupported_separately() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a");
        seed(&db, "unknown:b");
        save(
            &db,
            "qoder:a",
            &cred("tk", &iso_from_ms(now_ms() + 5 * DAY_MS)),
        );
        save(&db, "unknown:b", &cred("tk", "2099-01-01T00:00:00Z"));

        let report =
            refresh_credentials(&db, &["qoder:a".to_string(), "unknown:b".to_string()]).unwrap();

        assert_eq!(report.total, 2);
        assert_eq!(report.unsupported, 1);
        assert_eq!(report.up_to_date, 1);
        assert_eq!(report.failed, 0);
        assert!(report.errors.is_empty());
    }

    /// 批量：没有凭据的账号算失败，但不能中断其它账号
    #[test]
    fn batch_missing_credential_does_not_stop_the_rest() {
        let (_dir, db) = setup();
        seed(&db, "qoder:none");
        seed(&db, "qoder:ok");
        save(
            &db,
            "qoder:ok",
            &cred("tk", &iso_from_ms(now_ms() + 5 * DAY_MS)),
        );

        let report =
            refresh_credentials(&db, &["qoder:none".to_string(), "qoder:ok".to_string()]).unwrap();

        assert_eq!(report.failed, 1);
        assert_eq!(report.up_to_date, 1);
        assert_eq!(report.errors.len(), 1);
    }
}
