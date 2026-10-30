//! 扫描编排：把各家 Provider 的产物写进账号库。
//!
//! 顺序原则：**扫到一个就落库一个**。先把凭据加密写进去，再处理下一个，
//! 这样内存里同时存在的明文 token 始终只有一个（v1 是整包解密，所有 token 同时在内存）。

use crate::app::accounts::{repo, AccountIdentity};
use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::providers::QoderProvider;
use crate::app::timeutil::now_ms;

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ScanReport {
    /// 扫到的账号总数（含已存在的）
    pub scanned: usize,
    /// 新增的（库里原来没有）
    pub added: usize,
    /// 每个平台的明细：平台标签 → 条数
    pub by_platform: std::collections::BTreeMap<String, usize>,
    pub errors: Vec<String>,
}

pub fn scan_all(db: &Db) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    let existing = repo::list(db)?;
    let known: std::collections::HashSet<String> = existing.iter().map(|v| v.iid.clone()).collect();

    for provider in [QoderProvider::new(false), QoderProvider::new(true)] {
        let label = provider.label();
        match provider.scan(|scanned| persist(db, scanned, &mut report, &known)) {
            Ok(count) => {
                *report.by_platform.entry(label.to_string()).or_insert(0) += count;
            }
            Err(e) => report.errors.push(format!("{label}：{}", e.message)),
        }
    }

    Ok(report)
}

fn persist(
    db: &Db,
    scanned: crate::app::providers::ScannedAccount,
    report: &mut ScanReport,
    known: &std::collections::HashSet<String>,
) -> Result<()> {
    let AccountIdentity { iid, .. } = scanned.identity.clone();
    repo::upsert_identity(db, &scanned.identity, now_ms())?;
    repo::set_secret(db, &iid, &scanned.secret, scanned.identity.expires_at)?;

    report.scanned += 1;
    if !known.contains(&iid) {
        report.added += 1;
    }
    // scanned 在这里释放：凭据明文不跨账号留存
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::providers::ScannedAccount;

    fn fake(iid: &str, secret: &[u8]) -> ScannedAccount {
        let (platform, account_id) = iid.split_once(':').unwrap();
        ScannedAccount {
            identity: AccountIdentity {
                iid: iid.into(),
                platform: platform.into(),
                platform_label: "Test".into(),
                account_id: account_id.into(),
                name: "测试账号".into(),
                requires_checkin: true,
                has_secret: true,
                ..Default::default()
            },
            secret: secret.to_vec(),
        }
    }

    #[test]
    fn persist_writes_identity_and_encrypted_secret() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let mut report = ScanReport::default();
        let known = std::collections::HashSet::new();

        persist(&db, fake("qoder:a", b"tok-a"), &mut report, &known).unwrap();
        persist(&db, fake("qoder:b", b"tok-b"), &mut report, &known).unwrap();

        assert_eq!(report.scanned, 2);
        assert_eq!(report.added, 2);

        let rows = repo::list(&db).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.has_secret));
        // 凭据必须能解回原文，且不是明文存储
        assert_eq!(repo::secret_of(&db, "qoder:a").unwrap().as_deref(), Some(&b"tok-a"[..]));
    }

    #[test]
    fn second_scan_does_not_count_as_added() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let known = std::collections::HashSet::new();
        persist(&db, fake("qoder:a", b"1"), &mut ScanReport::default(), &known).unwrap();

        // 模拟第二次扫描：库里已经有 qoder:a
        let known: std::collections::HashSet<String> =
            repo::list(&db).unwrap().into_iter().map(|r| r.iid).collect();
        let mut report = ScanReport::default();
        persist(&db, fake("qoder:a", b"2"), &mut report, &known).unwrap();

        assert_eq!(report.scanned, 1);
        assert_eq!(report.added, 0, "已存在的账号不算新增");
        assert_eq!(repo::secret_of(&db, "qoder:a").unwrap().as_deref(), Some(&b"2"[..]), "凭据应被刷新");
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
            println!("  {} · {} · 凭据 {}", row.platform_label, row.name, row.has_secret);
        }
    }
}
