//! 「刷新所有账号状态」：重扫客户端登录态 → 用凭据包校正到期时间 → 汇总。
//!
//! 解决的是「库里的状态和客户端实际情况对不上」：登录态已经被客户端刷新过、
//! 凭据被换掉、到期时间变了，而本地账号库还停在扫描那一刻。重扫拿到最新凭据，
//! 再用凭据包回填到期时间，界面上的「剩 N 天」「已过期」才可信。
//!
//! **不做 token 续期**：各家 refresh 协议随时会变，自己实现一套等于给自己埋雷；
//! 凭据真过期了就重新登录（详情页的「重新登录」），那条路走的是平台自己的登录页。

use super::{credential::Credential, repo, scan};
use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::timeutil::now_ms;

/// 到期预警窗口：与界面「剩 N 天」标黄的口径保持一致。
const EXPIRING_DAYS: i64 = 3;
const DAY_MS: i64 = 24 * 3600 * 1000;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct RefreshReport {
    /// 库内账号总数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
    /// 本次扫描到的账号数（含已存在的）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub scanned: usize,
    /// 新增的（库里原来没有）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub added: usize,
    /// 有凭据的账号数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub with_secret: usize,
    /// 缺凭据的账号数：这些账号切不过去，要重新登录
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub missing_secret: usize,
    /// 凭据已过期
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub expired: usize,
    /// 凭据在预警窗口内到期
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub expiring: usize,
    /// 到期时间被校正的账号数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated: usize,
    /// 客户端当前登录的账号数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub current: usize,
    /// 读取失败：整家平台没扫到，或单个账号凭据解不开
    pub errors: Vec<String>,
    /// 汇总时刻
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at: i64,
    /// 整次刷新的耗时
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ms: u64,
}

/// 重扫 + 校正 + 汇总。会读客户端文件与解密凭据，调用方负责放到阻塞线程里。
pub fn refresh_all(db: &Db) -> Result<RefreshReport> {
    let started = std::time::Instant::now();

    // 1) 重扫客户端：凭据信息唯一可靠的更新来源。只有客户端当前登录的账号
    //    会被刷新到最新凭据，其余账号保持原样（契约 21：重扫不得清空已有数据）。
    let scan_report = scan::scan_all(db)?;

    // 2) 用凭据包校正到期时间等展示状态
    let mut report = sync_from_credentials(db)?;
    report.scanned = scan_report.scanned;
    report.added = scan_report.added;
    // 整家平台没扫到的错误排前面：它比单个账号凭据损坏更该先看
    let mut errors = scan_report.errors;
    errors.append(&mut report.errors);
    report.errors = errors;

    // 3) 当前登录：界面靠它标「当前」，切换或重登之后必须重读
    report.current = super::current_iids().len();
    report.ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// 逐个账号解密凭据包，把包里的到期时间回填到账号行，并统计缺凭据 / 过期 / 临期。
///
/// 逐个解密、逐个释放：内存里同时存在的明文凭据永远只有一个。单账号失败只记进
/// `errors`，不中断整批——「有一个账号的包坏了」不该让其余账号也刷不出来。
fn sync_from_credentials(db: &Db) -> Result<RefreshReport> {
    let mut report = RefreshReport::default();
    let rows = repo::list(db)?;
    report.total = rows.len();

    for row in rows {
        let bytes = match repo::secret_of(db, &row.iid) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                report.missing_secret += 1;
                continue;
            }
            Err(e) => {
                report.errors.push(format!(
                    "{}：凭据解不开（{}）",
                    row.display_name(),
                    e.message
                ));
                continue;
            }
        };
        report.with_secret += 1;

        let cred = match Credential::parse(&bytes) {
            Ok(cred) => cred,
            Err(e) => {
                report
                    .errors
                    .push(format!("{}：{}", row.display_name(), e.message));
                continue;
            }
        };
        // 凭据包里没有到期时间 = 「不知道」，不能拿来抹掉已知值
        let Some(expires_at) = cred.expires_ms() else {
            continue;
        };

        // 库里没有、或与凭据包不一致时才写：这是展示缓存，重复刷新不该产生写盘
        if row.expires_at != Some(expires_at) {
            match repo::set_expires_at(db, &row.iid, expires_at) {
                Ok(()) => report.updated += 1,
                Err(e) => {
                    report.errors.push(format!(
                        "{}：写入到期时间失败（{}）",
                        row.display_name(),
                        e.message
                    ));
                    continue;
                }
            }
        }

        let now = now_ms();
        if expires_at <= now {
            report.expired += 1;
        } else if expires_at - now <= EXPIRING_DAYS * DAY_MS {
            report.expiring += 1;
        }
    }

    report.at = now_ms();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::accounts::AccountIdentity;

    fn setup() -> (tempfile::TempDir, Db) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        (dir, db)
    }

    fn seed(db: &Db, iid: &str, expires_at: Option<i64>) {
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
        if let Some(ms) = expires_at {
            repo::set_expires_at(db, iid, ms).unwrap();
        }
    }

    fn secret_with_expiry(expires_at: &str) -> Vec<u8> {
        Credential {
            token: "tk".into(),
            expires_at: expires_at.into(),
            ..Default::default()
        }
        .to_bytes()
        .unwrap()
    }

    #[test]
    fn backfills_expiry_from_the_credential_package() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", None);
        repo::set_secret(
            &db,
            "qoder:a",
            &secret_with_expiry("2099-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let report = sync_from_credentials(&db).unwrap();

        assert_eq!(report.updated, 1, "库里没有到期时间时必须回填");
        let row = repo::list(&db).unwrap().remove(0);
        assert_eq!(
            row.expires_at,
            crate::app::timeutil::parse_ts(&serde_json::json!("2099-01-01T00:00:00Z"))
        );
    }

    #[test]
    fn stale_expiry_is_corrected_to_the_credential_value() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", Some(1_000));
        repo::set_secret(
            &db,
            "qoder:a",
            &secret_with_expiry("2099-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let report = sync_from_credentials(&db).unwrap();

        assert_eq!(report.updated, 1, "凭据包才是真相，旧值要纠正");
        assert_ne!(repo::list(&db).unwrap()[0].expires_at, Some(1_000));
    }

    #[test]
    fn second_refresh_writes_nothing_when_already_in_sync() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", None);
        repo::set_secret(
            &db,
            "qoder:a",
            &secret_with_expiry("2099-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        assert_eq!(sync_from_credentials(&db).unwrap().updated, 1);
        assert_eq!(
            sync_from_credentials(&db).unwrap().updated,
            0,
            "值没变就不该再写一次"
        );
    }

    #[test]
    fn accounts_without_secret_are_counted_not_failed() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", None);
        seed(&db, "qoder:b", None);
        repo::set_secret(
            &db,
            "qoder:a",
            &secret_with_expiry("2099-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let report = sync_from_credentials(&db).unwrap();

        assert_eq!(report.total, 2);
        assert_eq!(report.with_secret, 1);
        assert_eq!(report.missing_secret, 1, "缺凭据要单独计数");
        assert!(report.errors.is_empty(), "缺凭据不是错误");
    }

    #[test]
    fn corrupt_secret_is_reported_and_does_not_stop_the_batch() {
        let (_dir, db) = setup();
        seed(&db, "qoder:bad", None);
        seed(&db, "qoder:good", None);
        repo::set_secret(&db, "qoder:bad", b"not-json", None).unwrap();
        repo::set_secret(
            &db,
            "qoder:good",
            &secret_with_expiry("2099-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let report = sync_from_credentials(&db).unwrap();

        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.updated, 1, "坏的那一个不能拖住好的那一个");
    }

    #[test]
    fn expired_and_expiring_are_counted_from_the_credential() {
        let (_dir, db) = setup();
        seed(&db, "qoder:old", None);
        seed(&db, "qoder:soon", None);
        seed(&db, "qoder:fresh", None);
        repo::set_secret(
            &db,
            "qoder:old",
            &secret_with_expiry("2000-01-01T00:00:00Z"),
            None,
        )
        .unwrap();
        let soon = crate::app::accounts::credential::iso_from_ms(now_ms() + DAY_MS);
        repo::set_secret(&db, "qoder:soon", &secret_with_expiry(&soon), None).unwrap();
        repo::set_secret(
            &db,
            "qoder:fresh",
            &secret_with_expiry("2099-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let report = sync_from_credentials(&db).unwrap();

        assert_eq!(report.expired, 1);
        assert_eq!(report.expiring, 1);
        assert_eq!(report.with_secret, 3);
    }

    #[test]
    fn unknown_expiry_is_left_alone() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", Some(5_000));
        // 凭据包没有 expires_at（v1 用空串表示「不知道」）
        repo::set_secret(&db, "qoder:a", &secret_with_expiry(""), None).unwrap();

        let report = sync_from_credentials(&db).unwrap();

        assert_eq!(report.updated, 0);
        assert_eq!(
            repo::list(&db).unwrap()[0].expires_at,
            Some(5_000),
            "不知道不等于没有，不能抹掉已知值"
        );
    }
}
