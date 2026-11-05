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

/// 平台侧今天已经签过（用户在官方客户端签的）时，「刷新积分」必须把本地去重标记补上。
/// 不补的话：列表永远停在「待查询」，每轮轮询还会再问一次签到接口。
#[tokio::test]
async fn refresh_marks_today_when_platform_already_signed() {
    let paths = TestPaths::new();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid = seed_trae_account(&db);
    let http = Arc::new(MockHttp::new());
    http.on(
        STATUS_URL,
        200,
        r#"{"checked_in":true,"enable":true,"credits":10}"#,
    );
    http.on(
        USAGE_URL,
        200,
        r#"{"usage_summary":{"total_amount":100,"consumed_amount":40}}"#,
    );
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http.clone(), clock.clone());

    let store = ai_manager::app::checkin::SqliteCheckin::new(&db);
    assert!(
        !store.marked_today(&iid, clock.as_ref()),
        "前提：本工具没有这个账号的签到记录"
    );

    // 「刷新积分」按钮打的就是这一发
    let out = service.credits_batch(std::slice::from_ref(&iid)).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.balance, Some(60.0), "余额照旧要刷出来");
    assert!(
        store.marked_today(&iid, clock.as_ref()),
        "平台说今天已签，本地标记必须补上"
    );

    let mut rows = repo::list(&db).unwrap();
    ai_manager::app::checkin::store::hydrate(&db, &mut rows, clock.as_ref());
    assert_eq!(
        rows[0].checked_in,
        Some(true),
        "列表要显示「已签」，不是「待查询」"
    );
    assert_eq!(http.call_count(CLAIM_URL), 0, "状态已签就别再调签到接口");
}

/// 平台明确说「今天没签」时，列表要显示「未签」——不能也画成「待查询」。
/// 「待查询」只留给平台没给结论（活动未开放 / 查询失败）的那一种。
#[tokio::test]
async fn status_records_negative_checkin_so_list_is_not_unknown() {
    let paths = TestPaths::new();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid = seed_trae_account(&db);
    // mock_http 的 status 返回 checked_in=false
    let http = mock_http(200, r#"{"code":0}"#);
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http.clone(), clock.clone());

    let status = service.status(&iid).await.unwrap();
    assert_eq!(status.checked_in, Some(false));

    let mut rows = repo::list(&db).unwrap();
    ai_manager::app::checkin::store::hydrate(&db, &mut rows, clock.as_ref());
    assert_eq!(
        rows[0].checked_in,
        Some(false),
        "要说「未签」，不是「待查询」"
    );
}

const ZCODE_LIMIT_URL: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
const ZCODE_SUBSCRIPTION_URL: &str = "https://open.bigmodel.cn/api/biz/subscription/list";
const QUOTA_20: &str = r#"{"code":200,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":1,"usage":100,"currentValue":20}]}}"#;
const QUOTA_80: &str = r#"{"code":200,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":1,"usage":100,"currentValue":80}]}}"#;

fn seed_zcode_account(db: &Db) -> String {
    let iid = "zcode:u1".to_string();
    let identity = AccountIdentity {
        iid: iid.clone(),
        platform: "zcode".into(),
        platform_label: "ZCode".into(),
        account_id: "u1".into(),
        name: "测试账号".into(),
        expires_at: None,
        source: "credentials.json".into(),
        requires_checkin: false,
        has_secret: true,
    };
    repo::upsert_identity(db, &identity, 0).unwrap();
    // config.provider 里的明文 apiKey 是 candidate_tokens 唯一不走 DPAPI 的通道
    let secret = br#"{"token":"","meta":{"config":{"provider":{"p1":{"name":"coding-plan","options":{"apiKey":"plain-test-api-key-0123456789"}}}}}}"#;
    repo::set_secret(db, &iid, secret, None).unwrap();
    iid
}

/// 免签到平台走 status() 时，新额度也必须落库：列表数值只认 credit_view，
/// 缓存停在旧百分比就等于这次刷新白做了。
#[tokio::test]
async fn status_refreshes_quota_cache_for_quota_only_platform() {
    let paths = TestPaths::new();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid = seed_zcode_account(&db);
    let http = Arc::new(MockHttp::new());
    http.on_sequence(ZCODE_LIMIT_URL, vec![(200, QUOTA_20), (200, QUOTA_80)]);
    http.on(ZCODE_SUBSCRIPTION_URL, 200, r#"{"code":200,"data":[]}"#);
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http.clone(), clock.clone());
    let store = ai_manager::app::checkin::SqliteCheckin::new(&db);

    let snap = service.credits(&iid).await.unwrap();
    assert!(snap.quota.is_some(), "额度型账号应拿到 quota");
    assert_eq!(
        store.view_of(&iid).unwrap().quota.values().next(),
        Some(&20.0)
    );

    // 平台侧额度已变成 80%，这一发不能把新值丢掉
    service.status(&iid).await.unwrap();
    assert_eq!(
        store.view_of(&iid).unwrap().quota.values().next(),
        Some(&80.0),
        "status 也必须把新额度写进 credit_view 缓存"
    );

    // 契约 21：列表数值从库里读，不从返回值里攒
    let mut rows = repo::list(&db).unwrap();
    ai_manager::app::checkin::store::hydrate(&db, &mut rows, clock.as_ref());
    assert_eq!(rows[0].quota.values().next(), Some(&80.0));
    assert_eq!(http.call_count(ZCODE_LIMIT_URL), 2, "不能为刷新多打接口");
}

/// 「成功但没拿到数值」和「查询失败」必须能区分，界面只认 error 字段。
#[tokio::test]
async fn credits_batch_marks_only_real_failures() {
    let paths = TestPaths::new();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let ok_iid = seed_trae_account(&db);
    let bad_iid = "trae:u2".to_string();
    repo::upsert_identity(
        &db,
        &AccountIdentity {
            iid: bad_iid.clone(),
            platform: "trae".into(),
            platform_label: "Trae CN".into(),
            account_id: "u2".into(),
            name: "无凭据账号".into(),
            expires_at: None,
            source: "storage.json".into(),
            requires_checkin: true,
            has_secret: true,
        },
        0,
    )
    .unwrap();
    // 故意不写 secret_blob：load() 会明确报「没有保存凭据」

    let http = mock_http(200, r#"{"code":0}"#);
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http.clone(), clock.clone());

    let out = service
        .credits_batch(&[ok_iid.clone(), bad_iid.clone()])
        .await;
    assert_eq!(out.len(), 2);
    let ok = out.iter().find(|(iid, _)| iid == &ok_iid).unwrap();
    let bad = out.iter().find(|(iid, _)| iid == &bad_iid).unwrap();
    assert!(ok.1.error.is_none(), "查询成功不能标成失败");
    assert_eq!(ok.1.balance, Some(60.0));
    assert!(bad.1.error.is_some(), "真正的错误要有显式标记");
    assert!(bad.1.error.as_ref().unwrap().contains("没有保存凭据"));
}
