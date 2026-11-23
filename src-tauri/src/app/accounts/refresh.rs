//! 「刷新所有账号状态」：重扫客户端 → **云端优先**校验凭据 → 校正到期时间 → 汇总。
//!
//! 三步的分工：
//!   1. 重扫客户端只为**发现/更新**凭据（新账号、客户端续过的新 token），
//!      它不是状态真源；
//!   2. 本地拿不到可用期（未知 / 已过期 / 临期）的账号，先拿凭据问平台
//!      （见 `cloud.rs`）：能通就是有效，401 就是失效；
//!   3. 云端问不出来（没网、平台没有可查接口）才回退读本机文件——**兜底有计数**，
//!      界面能说清这一轮有几个账号是靠本机数据判的（`cloud_fallback`）。
//!
//! **不做 token 续期**：各家 refresh 协议都没有公开，自己拼请求等于把账号交给
//! 猜出来的端点；凭据真过期了就重新授权（详情页的「重新授权」），那条路走的是
//! 平台自己的登录页。

use super::cloud::{self, CloudVerdict};
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
    /// 新增的字段
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub added: usize,
    /// 有凭据的账号数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub with_secret: usize,
    /// 缺凭据的账号数：这些账号切不过去，要重新授权
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
    /// 云端确认凭据有效（本地给不出可用期的账号才会去问）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cloud_ok: usize,
    /// 云端明确判定凭据失效（401/403）：只能重新授权
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cloud_expired: usize,
    /// 云端问不出来（没网 / 平台没有可查接口），已回退读本机
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cloud_fallback: usize,
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
    refresh_with(db, true)
}

/// 只校验、不重扫入库的那一半。
///
/// 定时任务用它：关掉「启动扫描客户端」的用户不希望账号库被后台改写，
/// 但凭据的有效性与到期时间仍要每天更新。手动「刷新状态」照旧走 `refresh_all`。
pub fn refresh_status(db: &Db) -> Result<RefreshReport> {
    refresh_with(db, false)
}

fn refresh_with(db: &Db, scan_first: bool) -> Result<RefreshReport> {
    let started = std::time::Instant::now();

    // 1) 重扫客户端：凭据信息唯一可靠的更新来源。只有客户端当前登录的账号
    //    会被刷新到最新凭据，其余账号保持原样（契约 21：重扫不得清空已有数据）。
    let scan_report = if scan_first {
        Some(scan::scan_all(db)?)
    } else {
        None
    };

    // 2) 云端优先校验 + 用凭据包校正到期时间等展示状态
    let mut report = sync_with(db, &cloud::probe)?;
    if let Some(scan_report) = scan_report {
        report.scanned = scan_report.scanned;
        report.added = scan_report.added;
        // 整家平台没扫到的错误排前面：它比单个账号凭据损坏更该先看
        let mut errors = scan_report.errors;
        errors.append(&mut report.errors);
        report.errors = errors;
    }

    // 3) 当前登录：界面靠它标「当前」，切换或重授权之后必须重读
    report.current = super::current_iids().len();
    report.ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// 逐个账号解密凭据包，把包里的到期时间回填到账号行，并统计缺凭据 / 过期 / 临期。
///
/// `probe` 是云端校验入口：**只有本地给不出可用期的账号**才会调它，正常在有效期内的
/// 账号不会被这一轮刷新打一次接口（几十个账号就是几十次请求，没有必要）。
///
/// 逐个解密、逐个释放：内存里同时存在的明文凭据永远只有一个。单账号失败只记进
/// `errors`，不中断整批——「有一个账号的包坏了」不该让其余账号也刷不出来。
fn sync_with(db: &Db, probe: &dyn Fn(&str, &Credential) -> CloudVerdict) -> Result<RefreshReport> {
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
        let now = now_ms();
        let expires_at = cred.expires_ms();

        // 云端优先：本地给不出可用期（未知 / 已过期 / 临期）的账号先问平台。
        // 本地时间还在有效期内就不问——那是每次刷新几十次无谓请求的来源。
        let needs_cloud = expires_at.is_none_or(|ms| ms - now <= EXPIRING_DAYS * DAY_MS);
        let cloud_valid = if needs_cloud {
            match probe(&row.iid, &cred) {
                CloudVerdict::Valid => {
                    report.cloud_ok += 1;
                    true
                }
                CloudVerdict::Expired { .. } => {
                    report.cloud_expired += 1;
                    // 界面靠失败标记给出「需要重新授权」的下一步
                    let _ = repo::note_health_fail(db, &row.iid, "auth_expired", now);
                    false
                }
                CloudVerdict::Unavailable { .. } => {
                    report.cloud_fallback += 1;
                    false
                }
            }
        } else {
            false
        };

        // 凭据包里没有到期时间 = 「不知道」，不能拿来抹掉已知值
        let Some(expires_at) = expires_at else {
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

        if expires_at <= now {
            // 云端刚确认过这份凭据有效：本地这个时间只是访问令牌的（Qoder 每天顺延），
            // 不该把账号判成过期，按临期提醒即可
            if cloud_valid {
                report.expiring += 1;
            } else {
                report.expired += 1;
            }
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

    /// 测试里不联网：云端一律「问不出来」，走本机兜底这条主路径。
    fn no_cloud(_iid: &str, _cred: &Credential) -> CloudVerdict {
        CloudVerdict::Unavailable {
            reason: "测试不联网".into(),
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

        let report = sync_with(&db, &no_cloud).unwrap();

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

        let report = sync_with(&db, &no_cloud).unwrap();

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

        assert_eq!(sync_with(&db, &no_cloud).unwrap().updated, 1);
        assert_eq!(
            sync_with(&db, &no_cloud).unwrap().updated,
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

        let report = sync_with(&db, &no_cloud).unwrap();

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

        let report = sync_with(&db, &no_cloud).unwrap();

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

        let report = sync_with(&db, &no_cloud).unwrap();

        assert_eq!(report.expired, 1);
        assert_eq!(report.expiring, 1);
        assert_eq!(report.with_secret, 3);
    }

    #[test]
    fn unknown_expiry_is_left_alone() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", Some(5_000));
        // 凭据包没有 expires_at（空串表示「不知道」）
        repo::set_secret(&db, "qoder:a", &secret_with_expiry(""), None).unwrap();

        let report = sync_with(&db, &no_cloud).unwrap();

        assert_eq!(report.updated, 0);
        assert_eq!(
            repo::list(&db).unwrap()[0].expires_at,
            Some(5_000),
            "不知道不等于没有，不能抹掉已知值"
        );
    }

    /// 云端说有效时，本地那条已经过去的访问令牌时间不该把账号判成过期。
    #[test]
    fn cloud_valid_overrides_a_past_local_expiry() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", None);
        repo::set_secret(
            &db,
            "qoder:a",
            &secret_with_expiry("2000-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let report = sync_with(&db, &|_, _| CloudVerdict::Valid).unwrap();

        assert_eq!(report.cloud_ok, 1);
        assert_eq!(report.expired, 0, "云端确认有效就不是过期");
        assert_eq!(report.expiring, 1, "但要在界面提醒有效期已到");
    }

    /// 云端 401：明确记一次失败标记，界面据此给出「重新授权」。
    #[test]
    fn cloud_expired_records_a_failure_mark() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", None);
        // 凭据包没有到期时间 = 本地给不出可用期，才会去问云端
        repo::set_secret(&db, "qoder:a", &secret_with_expiry(""), None).unwrap();

        let report = sync_with(&db, &|_, _| CloudVerdict::Expired {
            reason: "401".into(),
        })
        .unwrap();

        assert_eq!(report.cloud_expired, 1);
        let row = repo::list(&db).unwrap().remove(0);
        assert_eq!(row.last_fail_kind.as_deref(), Some("auth_expired"));
    }

    /// 云端问不出来要单独计数：这一轮里哪些账号是靠本机数据判的，界面要说清。
    #[test]
    fn cloud_unavailable_is_counted_as_fallback() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", None);
        repo::set_secret(
            &db,
            "qoder:a",
            &secret_with_expiry("2000-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let report = sync_with(&db, &no_cloud).unwrap();

        assert_eq!(report.cloud_fallback, 1);
        assert_eq!(report.cloud_ok, 0);
        assert_eq!(report.expired, 1, "回退本机后照旧按本地时间判过期");
    }

    /// 还在有效期内的账号不该为一次「刷新状态」多打一次平台接口。
    #[test]
    fn valid_accounts_do_not_hit_the_cloud() {
        let (_dir, db) = setup();
        seed(&db, "qoder:a", None);
        repo::set_secret(
            &db,
            "qoder:a",
            &secret_with_expiry("2099-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let report = sync_with(&db, &|_, _| panic!("有效期内的账号不该问云端")).unwrap();

        assert_eq!(
            report.cloud_ok + report.cloud_expired + report.cloud_fallback,
            0
        );
    }
}
