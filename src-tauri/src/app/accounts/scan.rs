//! 扫描编排：遍历各平台 Provider，把产物**扫到一个就落库一个**。
//!
//! 顺序原则的原因：先把凭据加密写进去，再处理下一个，这样内存里同时存在的
//! 明文 token 始终只有一个（v1 是整包解密，所有 token 同时在内存）。

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
    /// 新增的（库里原来没有）
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
/// 旧版两个缺陷共同造出这种行：客户端 3.18 的 v2 加密解不开 → identity 全兜底 →
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
        if crate::app::providers::zcode::identity(&raw).0 == current {
            if repo::delete(db, &row.iid).unwrap_or(false) {
                crate::app::log::log(
                    crate::app::log::Level::Info,
                    "accounts",
                    "已清理历史缺陷留下的重复行「zcode:ZCode 账号」（同一登录已用正确身份入库）",
                );
            }
        }
    }
}

fn persist(db: &Db, scanned: ScannedAccount, user_platform: &str) -> Result<()> {
    let AccountIdentity {
        ref iid,
        ref platform,
        ..
    } = scanned.identity;
    debug_assert_eq!(
        platform, user_platform,
        "Provider 的 platform 必须与身份一致"
    );
    repo::upsert_identity(db, &scanned.identity, now_ms())?;
    repo::set_secret(db, iid, &scanned.secret, scanned.identity.expires_at)?;
    // scanned 在这里释放：凭据明文不跨账号留存
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::accounts::credential::Credential;

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
