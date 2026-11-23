//! 扫描编排：遍历各平台 Provider，把产物**扫到一个就落库一个**。
//!
//! 顺序原则的原因：先把凭据加密写进去，再处理下一个，这样内存里同时存在的
//! 明文 token 始终只有一个（整包解密会让所有 token 同时在内存）。

use crate::app::accounts::credential::Credential;
use crate::app::accounts::{repo, AccountIdentity};
use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::providers::{self, ScannedAccount};
use crate::app::timeutil::now_ms;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ScanReport {
    /// 扫到的账号总数（含已存在的）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub scanned: usize,
    /// 新增的字段
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub added: usize,
    /// 每个平台的明细：平台标签 → 条数
    pub by_platform: std::collections::BTreeMap<String, usize>,
    pub errors: Vec<String>,
}

pub fn scan_all(db: &Db) -> Result<ScanReport> {
    scan_providers(db, providers::all())
}

/// 只扫**一家**平台。刷新凭据用：为了一个账号把六家客户端全读一遍没有意义，
/// 而且其它平台的扫描失败会混进错误列表里，把「这个账号续不了」的结论搅浑。
pub fn scan_platform(db: &Db, client_key: &str) -> Result<ScanReport> {
    let provider = providers::for_platform(client_key).ok_or_else(|| {
        crate::app::error::AppError::new(
            crate::app::error::ErrorKind::Unsupported,
            format!("{client_key} 不是已知平台"),
        )
    })?;
    scan_providers(db, vec![provider])
}

fn scan_providers(db: &Db, providers: Vec<Box<dyn providers::Provider>>) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    let known: std::collections::HashSet<String> =
        repo::list(db)?.into_iter().map(|v| v.iid).collect();

    for provider in providers {
        let label = provider.label();
        let platform = provider.platform();
        let mut added = 0usize;
        let count = provider.scan(&mut |scanned| {
            let is_new = !known.contains(&scanned.identity.iid);
            persist(db, scanned, platform)?;
            if is_new {
                added += 1;
            }
            Ok(())
        });
        match count {
            Ok(count) => {
                *report.by_platform.entry(label.to_string()).or_insert(0) += count;
                report.scanned += count;
                report.added += added;
            }
            // 单个平台失败不拖垮整次扫描：其他平台照常入库，错误汇总给界面
            Err(e) => report.errors.push(format!("{label}：{}", e.message)),
        }
    }

    cleanup_legacy_zcode_rows(db);
    Ok(report)
}

/// 清理历史缺陷留下的「zcode:ZCode 账号」垃圾行。
///
/// 客户端 3.18 的 v2 加密解不开 → identity 全兜底 →
/// 「ZCode 账号」同时成了显示名和 account_id，每次扫描都会再插一行。修复后同一份
/// 登录会以正确身份入库，垃圾行留着只会一直「查不出积分」。**只删能确认与客户端
/// 当前登录是同一份的**（identity 解出来相等），其余保留给用户手动处理。
fn cleanup_legacy_zcode_rows(db: &Db) {
    const LEGACY_IID: &str = "zcode:ZCode 账号";
    let Ok(rows) = repo::list(db) else {
        return;
    };
    let current = providers::for_platform("zcode").and_then(|p| p.current_id());
    let Some(current) = current else {
        return;
    };
    for row in rows.iter().filter(|r| r.iid == LEGACY_IID && r.has_secret) {
        let bytes = match repo::secret_of(db, &row.iid) {
            Ok(Some(bytes)) => bytes,
            _ => continue,
        };
        let cred = match Credential::parse(&bytes) {
            Ok(cred) => cred,
            Err(_) => continue,
        };
        let Some(raw) = cred.meta_json("credentials") else {
            continue;
        };
        if crate::app::providers::zcode::identity(raw).0 == current
            && repo::delete(db, &row.iid).unwrap_or(false)
        {
            crate::app::log::log(
                crate::app::log::Level::Info,
                "accounts",
                "已清理历史缺陷留下的重复行「zcode:ZCode 账号」（同一登录已用正确身份入库）",
            );
        }
    }
}

fn persist(db: &Db, mut scanned: ScannedAccount, user_platform: &str) -> Result<()> {
    let AccountIdentity {
        ref iid,
        ref platform,
        ..
    } = scanned.identity;
    debug_assert_eq!(
        platform, user_platform,
        "Provider 的 platform 必须与身份一致"
    );
    let now = now_ms();

    // 客户端文件不一定是最新的一份：应用内「重新授权」只写本地库，客户端可能还停在
    // 更早的旧登录态（甚至就是授权前那份已过期的）。重扫若无条件覆盖，用户刚授权的
    // 凭据会被打回旧值——表现就是「授权白做了、到期一直显示已过期」。
    let local_expiry = repo::secret_of(db, iid)
        .ok()
        .flatten()
        .and_then(|bytes| Credential::parse(&bytes).ok())
        .and_then(|cred| cred.expires_ms());
    if keeps_local(local_expiry, scanned.identity.expires_at, now) {
        // 只更新身份字段（昵称 / 来源），凭据与到期时间保持本地那份
        scanned.identity.expires_at = local_expiry;
        repo::upsert_identity(db, &scanned.identity, now)?;
        crate::app::log::log(
            crate::app::log::Level::Warn,
            "accounts",
            format!(
                "{}：客户端登录态不比本地更新，保留本地凭据（本地到期 {}）",
                scanned.identity.name,
                local_expiry.map_or_else(
                    || "未知".to_string(),
                    crate::app::accounts::credential::iso_from_ms
                )
            ),
        );
        return Ok(());
    }

    repo::upsert_identity(db, &scanned.identity, now)?;
    repo::set_secret(db, iid, &scanned.secret, scanned.identity.expires_at)?;
    // scanned 在这里释放：凭据明文不跨账号留存
    Ok(())
}

/// 重扫时是否保留本地那份凭据（`true` = 客户端这份不作为准）。
///
/// 唯一原则：**客户端这份必须能证明自己更新**，否则不覆盖已授权进来的本地凭据。
/// 判据只用到期时间——它是唯一跨平台可比的「新旧」信号：
///   - 本地明确过期 → 没理由保留，用客户端的；
///   - 客户端明确过期、本地没过期（或有效期未知）→ 留着，别把授权打回旧状态；
///   - 两份都有效 → 明显更晚才换（1 小时内视为同一份，避免来回抖）；
///   - 客户端没给有效期 → 本地有效就留着，别把已知的有效期换成「未知」；
///   - 两边都没有效期 → 以客户端为准（ZCode 就是这种，token 只有客户端会续）。
fn keeps_local(local: Option<i64>, scanned: Option<i64>, now: i64) -> bool {
    const EXPIRY_ROLLBACK_SLACK_MS: i64 = 3600 * 1000;
    if local.is_some_and(|ms| ms <= now) {
        return false;
    }
    match (local, scanned) {
        (_, Some(s)) if s <= now => true,
        (None, Some(_)) => false,
        (Some(l), Some(s)) => l + EXPIRY_ROLLBACK_SLACK_MS >= s,
        (Some(_), None) => true,
        (None, None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::accounts::credential::{iso_from_ms, Credential};

    fn fake(iid: &str, secret: &str) -> ScannedAccount {
        let (platform, account_id) = iid.split_once(':').unwrap();
        let cred = Credential {
            token: secret.into(),
            ..Default::default()
        };
        ScannedAccount::new(
            AccountIdentity {
                iid: iid.into(),
                platform: platform.into(),
                platform_label: "Test".into(),
                account_id: account_id.into(),
                name: "测试账号".into(),
                requires_checkin: true,
                has_secret: true,
                ..Default::default()
            },
            &cred,
        )
        .unwrap()
    }

    #[test]
    fn persist_writes_identity_and_encrypted_secret() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        persist(&db, fake("qoder:a", "tok-a"), "qoder").unwrap();
        persist(&db, fake("qoder:b", "tok-b"), "qoder").unwrap();

        let rows = repo::list(&db).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.has_secret));
        // 凭据必须能解回原文，且不是明文存储
        let back = repo::secret_of(&db, "qoder:a").unwrap().unwrap();
        assert_eq!(Credential::parse(&back).unwrap().token, "tok-a");
    }

    #[test]
    fn second_scan_refreshes_secret_without_duplicating() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        persist(&db, fake("qoder:a", "1"), "qoder").unwrap();

        persist(&db, fake("qoder:a", "2"), "qoder").unwrap();

        assert_eq!(repo::list(&db).unwrap().len(), 1, "同一账号不能变成两行");
        let back = repo::secret_of(&db, "qoder:a").unwrap().unwrap();
        assert_eq!(Credential::parse(&back).unwrap().token, "2", "凭据应被刷新");
    }

    fn fake_with_expiry(iid: &str, token: &str, expires_at: i64) -> ScannedAccount {
        let (platform, account_id) = iid.split_once(':').unwrap();
        let cred = Credential {
            token: token.into(),
            expires_at: iso_from_ms(expires_at),
            ..Default::default()
        };
        ScannedAccount::new(
            AccountIdentity {
                iid: iid.into(),
                platform: platform.into(),
                platform_label: "Test".into(),
                account_id: account_id.into(),
                name: "测试账号".into(),
                expires_at: Some(expires_at),
                requires_checkin: true,
                has_secret: true,
                ..Default::default()
            },
            &cred,
        )
        .unwrap()
    }

    /// 重新授权把新凭据写进库里之后，客户端文件可能还停在更早的旧登录态。
    /// 重扫不能拿那份更早的凭据把库里更晚的有效期覆盖掉。
    #[test]
    fn stale_scan_does_not_roll_back_a_longer_valid_expiry() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let now = now_ms();
        // Credential 里的 ISO 到期时间精确到秒，测试基准也先对齐到秒
        let newer = (now + 50 * 24 * 3600 * 1000) / 1000 * 1000;
        let older = (now + 3 * 3600 * 1000) / 1000 * 1000;

        persist(&db, fake_with_expiry("qoder:a", "new", newer), "qoder").unwrap();
        persist(&db, fake_with_expiry("qoder:a", "old", older), "qoder").unwrap();

        let back = repo::secret_of(&db, "qoder:a").unwrap().unwrap();
        let cred = Credential::parse(&back).unwrap();
        assert_eq!(cred.token, "new", "更早的客户端凭据不能覆盖更新的凭据");
        assert_eq!(cred.expires_ms(), Some(newer));
        assert_eq!(repo::list(&db).unwrap()[0].expires_at, Some(newer));
    }

    /// 授权后客户端仍停在授权前的旧（已过期）登录态：重扫不能把刚授权的凭据打回去。
    /// 这就是「授权完接着扫一次本机，等于白授权」的现场。
    #[test]
    fn expired_client_credential_does_not_overwrite_a_fresh_authorization() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let now = now_ms();
        let future = (now + 30 * 24 * 3600 * 1000) / 1000 * 1000;
        let past = (now - 2 * 24 * 3600 * 1000) / 1000 * 1000;

        persist(
            &db,
            fake_with_expiry("workbuddy:u1", "authorized", future),
            "workbuddy",
        )
        .unwrap();
        persist(
            &db,
            fake_with_expiry("workbuddy:u1", "stale", past),
            "workbuddy",
        )
        .unwrap();

        let cred =
            Credential::parse(&repo::secret_of(&db, "workbuddy:u1").unwrap().unwrap()).unwrap();
        assert_eq!(
            cred.token, "authorized",
            "客户端那份过期凭据不能顶掉刚授权的"
        );
        assert_eq!(cred.expires_ms(), Some(future));
        assert_eq!(repo::list(&db).unwrap()[0].expires_at, Some(future));
    }

    /// 本地有效期未知（平台不返回 exp）同样不能被客户端的过期凭据顶掉——
    /// 这种凭据多半是网页授权刚拿到的，顶掉之后连「到期」都显示不出来。
    #[test]
    fn unknown_local_expiry_survives_an_expired_scan() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let now = now_ms();
        let past = (now - 24 * 3600 * 1000) / 1000 * 1000;

        persist(&db, fake("zcode:u1", "authorized"), "zcode").unwrap();
        persist(&db, fake_with_expiry("zcode:u1", "stale", past), "zcode").unwrap();

        let cred = Credential::parse(&repo::secret_of(&db, "zcode:u1").unwrap().unwrap()).unwrap();
        assert_eq!(cred.token, "authorized");
        assert_eq!(cred.expires_ms(), None, "不能凭空写入客户端的过期时间");
    }

    /// 本地确实过期的账号：客户端的有效凭据才是新的，必须换过来，否则永远卡在过期。
    #[test]
    fn valid_client_credential_replaces_an_expired_local_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let now = now_ms();
        let past = (now - 5 * 24 * 3600 * 1000) / 1000 * 1000;
        let future = (now + 20 * 24 * 3600 * 1000) / 1000 * 1000;

        persist(
            &db,
            fake_with_expiry("workbuddy:u1", "expired", past),
            "workbuddy",
        )
        .unwrap();
        persist(
            &db,
            fake_with_expiry("workbuddy:u1", "renewed", future),
            "workbuddy",
        )
        .unwrap();

        let cred =
            Credential::parse(&repo::secret_of(&db, "workbuddy:u1").unwrap().unwrap()).unwrap();
        assert_eq!(cred.token, "renewed");
        assert_eq!(cred.expires_ms(), Some(future));
    }

    /// ZCode 这类两端都没有 exp 的平台：token 只有客户端会续，必须以客户端为准。
    #[test]
    fn no_expiry_on_either_side_takes_the_client_copy() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        persist(&db, fake("zcode:u1", "old"), "zcode").unwrap();
        persist(&db, fake("zcode:u1", "renewed"), "zcode").unwrap();
        let cred = Credential::parse(&repo::secret_of(&db, "zcode:u1").unwrap().unwrap()).unwrap();
        assert_eq!(cred.token, "renewed");
    }

    /// 真实机验证：`cargo test --lib scan_all_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scan_all_real_machine() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let report = scan_all(&db).unwrap();
        println!("扫描结果：{report:?}");
        for row in repo::list(&db).unwrap() {
            println!(
                "  {} · {} · 凭据 {}",
                row.platform_label, row.name, row.has_secret
            );
        }
    }
}
