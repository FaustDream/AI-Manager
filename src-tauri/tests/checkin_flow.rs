//! 签到编排的端到端断言：**幂等防线到底有没有拦住第二次调用**。
//!
//! 契约 2 在 `contracts.rs` 里只验了去重表本身；这里验的是「表 → 编排 → 网络」
//! 整条链路——重复调用一次的代价由用户承担，所以必须用 `MockHttp.calls()` 硬断言。

mod support;

use ai_manager::app::accounts::{repo, AccountIdentity};
use ai_manager::app::checkin::Service;
use ai_manager::app::db::Db;
use ai_manager::app::net::MockHttp;
use ai_manager::contracts::CheckinStore;
use std::sync::Arc;
use support::*;

const STATUS_URL: &str = "https://api.trae.cn/trae/api/v2/ug/checkin_credits/status";
const CLAIM_URL: &str = "https://api.trae.cn/trae/api/v2/ug/checkin_credits/claim";
const USAGE_URL: &str = "https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage";

fn seed_trae_account(db: &Db) -> String {
    let iid = "trae:u1".to_string();
    let identity = AccountIdentity {
        iid: iid.clone(),
        platform: "trae".into(),
        platform_label: "Trae CN".into(),
        account_id: "u1".into(),
        name: "测试账号".into(),
        expires_at: None,
        source: "storage.json".into(),
        requires_checkin: true,
        has_secret: true,
    };
    repo::upsert_identity(db, &identity, 0).unwrap();
    // 凭据包里只要有 token 就够：Trae 的请求靠它拼 Authorization
    let secret = br#"{"token":"jwt-live","expires_at":"","meta":{"uid":"u1","region":"cn","device_id":"d1"}}"#;
    repo::set_secret(db, &iid, secret, None).unwrap();
    iid
}

fn mock_http(claim_status: u16, claim_body: &str) -> Arc<MockHttp> {
    let http = Arc::new(MockHttp::new());
    http.on(
        STATUS_URL,
        200,
        r#"{"checked_in":false,"enable":true,"credits":10}"#,
    );
    http.on(CLAIM_URL, claim_status, claim_body);
    http.on(
        USAGE_URL,
        200,
        r#"{"usage_summary":{"total_amount":100,"consumed_amount":40}}"#,
    );
    http
}

#[tokio::test]
async fn second_checkin_does_not_call_the_api_again() {
    let paths = TestPaths::new();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid = seed_trae_account(&db);
    let http = mock_http(200, r#"{"code":0}"#);
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http.clone(), clock.clone());

    let first = service.checkin(&iid).await.unwrap();
    assert!(first.ok, "第一次应成功：{}", first.message);
    assert!(!first.already);
    assert_eq!(http.call_count(CLAIM_URL), 1, "第一次必须真的调一次");

    // 同一天再点一次：去重表挡住了，接口一次都不能再打
    let second = service.checkin(&iid).await.unwrap();
    assert!(second.ok);
    assert!(second.already, "第二次应标记为「今日已签」");
    assert_eq!(
        http.call_count(CLAIM_URL),
        1,
        "重复签到绝不能再打接口——这一天会被平台扣两次"
    );
}

#[tokio::test]
async fn failed_checkin_does_not_mark_today() {
    let paths = TestPaths::new();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid = seed_trae_account(&db);
    // 第一次失败，第二次成功后必须还能重试
    let http = Arc::new(MockHttp::new());
    http.on(
        STATUS_URL,
        200,
        r#"{"checked_in":false,"enable":true,"credits":10}"#,
    );
    http.on_sequence(
        CLAIM_URL,
        vec![
            (500, r#"{"message":"服务器开小差"}"#),
            (200, r#"{"code":0}"#),
        ],
    );
    http.on(
        USAGE_URL,
        200,
        r#"{"usage_summary":{"total_amount":100,"consumed_amount":40}}"#,
    );
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http.clone(), clock.clone());

    // 接口 500：编排层如实把错误往上抛（界面按 kind 给话术），关键是别记成已签
    let failed = service.checkin(&iid).await;
    assert!(failed.is_err(), "接口失败要如实报错，不能静默成功");

    let retried = service.checkin(&iid).await.unwrap();
    assert!(
        retried.ok,
        "失败不写去重表，第二次应当还能签：{}",
        retried.message
    );
    assert_eq!(http.call_count(CLAIM_URL), 2, "失败那次不占去重名额");
}

#[tokio::test]
async fn status_records_balance_and_next_at() {
    let paths = TestPaths::new();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid = seed_trae_account(&db);
    let http = mock_http(200, r#"{"code":0}"#);
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http.clone(), clock.clone());

    let status = service.status(&iid).await.unwrap();
    // Trae 的余额来自用量接口：100 - 40
    assert_eq!(status.balance, Some(60.0));

    // 契约 6：余额写进快照表，重启后仍能读到
    let store = ai_manager::app::checkin::SqliteCheckin::new(&db);
    assert_eq!(store.balance_on(&iid, "2026-10-30"), Some(60.0));
    // 契约 21：列表重读时余额从库里补出来，不需要再问一次接口
    let mut rows = repo::list(&db).unwrap();
    ai_manager::app::checkin::store::hydrate(&db, &mut rows, clock.as_ref());
    assert_eq!(rows[0].balance, Some(60.0), "重启后列表仍要显示上次余额");
}
