//! 把 CodeBuddy 两端（IDE / IDEA 插件）已登录的号收进账号库。
//!
//! 端不是独立账号体系：它登的就是 WorkBuddy 国内版账号，会话里存着 WorkBuddy 的
//! 票据（[`codebuddy::EndLogin`]）。所以「在 IDE 里登录一个号」在本工具看来就是
//! 「库里多了一个 WorkBuddy 账号」——收进来之后，列表里那一行端才有账号可挂，
//! 也才能对它签到、查积分、参与轮询。
//!
//! 反过来的方向（把库里的号写进端）在 `switch_codebuddy`，两边共用同一份票据。
//!
//! **不覆盖已有账号的凭据**：库里已经有这个 uid 时直接跳过。端上的票据可能比
//! 账号库里存的更旧（用户先在网页登录过、之后才在 IDE 登录），拿它盖回去等于
//! 把一条能用的凭据换成一条快要过期的。

use crate::app::accounts::credential::Credential;
use crate::app::accounts::login;
use crate::app::accounts::repo;
use crate::app::accounts::view::AccountIdentity;
use crate::app::accounts::ScanReport;
use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::products::Product;
use crate::app::providers::codebuddy::{self, Target};
use serde_json::json;

/// 扫一遍两个端，把端上登录、库里还没有的号收进账号库。
///
/// 失败只记账不中断扫描：收编是扫描的**附加**收益，不能让一条读不出来的端
/// 把其它平台的扫描结果一起带掉。
pub fn adopt_codebuddy(db: &Db, roots: &Roots, report: &mut ScanReport) {
    for target in [Target::Software, Target::IdeaPlugin] {
        let Some(end) = codebuddy::current_login(roots, target) else {
            continue;
        };
        // 国际版票据写不进 WorkBuddy 国内接口，收进来只是一条查不出数的号
        if end.edition != "cn" {
            continue;
        }
        let iid = format!("workbuddy:{}", end.uid);
        match repo::identity(db, &iid) {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(e) => {
                report.errors.push(format!("读取账号库失败：{}", e.message));
                continue;
            }
        }

        let (source, label) = match target {
            Target::Software => ("codebuddy_software", Product::CodeBuddyIde.label()),
            Target::IdeaPlugin => ("codebuddy_idea", Product::CodeBuddyIdea.label()),
        };
        let identity = AccountIdentity {
            iid: iid.clone(),
            platform: "workbuddy".into(),
            platform_label: "WorkBuddy".into(),
            account_id: end.uid.clone(),
            name: end.name.clone(),
            expires_at: end.expires_at,
            source: source.into(),
            // 收进来的是完整 WorkBuddy 账号，签到规则与扫描进来的一样
            requires_checkin: true,
            has_secret: true,
        };
        let mut meta = serde_json::Map::new();
        meta.insert("uid".into(), json!(end.uid));
        meta.insert("domain".into(), json!(end.domain));
        meta.insert("edition".into(), json!("cn"));
        meta.insert("ua".into(), json!(login::WB_UA));
        meta.insert("refresh_token".into(), json!(end.refresh));
        // 留痕：凭据是从哪个端读来的，出问题时能一眼对上是哪一份
        meta.insert("adopted_from".into(), json!(source));
        let cred = Credential {
            token: end.token,
            refresh_token: end.refresh,
            expires_at: login::iso_of(end.expires_at),
            meta,
        };

        match login::save(db, identity, &cred, end.expires_at, &end.name) {
            Ok(_) => {
                report.scanned += 1;
                report.added += 1;
                *report.by_platform.entry("WorkBuddy".into()).or_insert(0) += 1;
                crate::app::log::log(
                    crate::app::log::Level::Info,
                    "accounts",
                    format!("从 {label} 收编账号：{}", end.name),
                );
            }
            Err(e) => report
                .errors
                .push(format!("{label} 上登录的账号收编失败：{}", e.message)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::accounts::{credential::Credential, repo, ScanReport};
    use crate::app::db::Db;
    use serde_json::json;

    /// 插件端把会话明文存在 `secret-storage.xml` 的 Value 里（XML 转义后的 JSON）。
    fn idea_store(dir: &std::path::Path, uid: &str, domain: &str) {
        idea_store_with(dir, uid, domain, "tok-A");
    }

    fn idea_store_with(dir: &std::path::Path, uid: &str, domain: &str, token: &str) {
        let home = dir.join("JetBrains").join("IntelliJIdea2026.3");
        std::fs::create_dir_all(home.join("plugins").join("codebuddy")).unwrap();
        std::fs::create_dir_all(home.join("options")).unwrap();
        let session = json!({
            "account": { "uid": uid, "nickname": "凌致" },
            "auth": {
                "accessToken": token,
                "refreshToken": "ref-A",
                "expiresAt": 1796347317300i64,
                "domain": domain,
            }
        });
        let escaped = serde_json::to_string(&session)
            .unwrap()
            .replace('&', "&amp;")
            .replace('"', "&quot;");
        std::fs::write(
            home.join("options").join("secret-storage.xml"),
            format!(
                r#"<Scores><Entry key="Tencent-Cloud.coding-copilot.new.accessToken" value="{escaped}" /></Scores>"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn adopts_the_account_the_plugin_is_logged_into() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        idea_store(dir.path(), "u-9", "www.workbuddy.cn");

        let mut report = ScanReport::default();
        adopt_codebuddy(
            &db,
            &Roots::at(dir.path(), dir.path(), dir.path()),
            &mut report,
        );

        assert_eq!(report.added, 1, "{:?}", report.errors);
        let identity = repo::identity(&db, "workbuddy:u-9").unwrap().unwrap();
        assert_eq!(identity.name, "凌致");
        assert_eq!(identity.platform, "workbuddy");
        assert_eq!(identity.source, "codebuddy_idea");
        let cred =
            Credential::parse(&repo::secret_of(&db, "workbuddy:u-9").unwrap().unwrap()).unwrap();
        assert_eq!(cred.token, "tok-A", "收进来的就是端上那份 WorkBuddy 票据");
        assert_eq!(cred.refresh_token, "ref-A");
        assert_eq!(cred.meta_str("domain"), Some("www.workbuddy.cn"));
    }

    #[test]
    fn does_not_overwrite_an_account_that_is_already_known() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        idea_store(dir.path(), "u-9", "www.workbuddy.cn");
        let roots = Roots::at(dir.path(), dir.path(), dir.path());

        let mut first = ScanReport::default();
        adopt_codebuddy(&db, &roots, &mut first);
        assert_eq!(first.added, 1);

        // 端上换了一份票据：库里已有这一条就不该被它盖掉
        idea_store_with(dir.path(), "u-9", "www.workbuddy.cn", "tok-B");

        let mut second = ScanReport::default();
        adopt_codebuddy(&db, &roots, &mut second);
        assert_eq!(second.added, 0);
        let cred =
            Credential::parse(&repo::secret_of(&db, "workbuddy:u-9").unwrap().unwrap()).unwrap();
        assert_eq!(cred.token, "tok-A", "已有账号的凭据不能被端上的票据覆盖");
    }

    #[test]
    fn ignores_international_sessions() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        idea_store(dir.path(), "u-9", "www.workbuddy.ai");

        let mut report = ScanReport::default();
        adopt_codebuddy(
            &db,
            &Roots::at(dir.path(), dir.path(), dir.path()),
            &mut report,
        );

        assert_eq!(report.added, 0);
        assert!(repo::identity(&db, "workbuddy:u-9").unwrap().is_none());
    }
}
