//! 「刷新积分」失败必须逐条留痕：前端 toast 里的「N 个查询失败」必须有后端 Error 行对应。
//!
//! 以前 `credits_batch` 把错误塞进 `CreditSnapshot.error` 就返回，界面能看到红字，
//! 但 `app.jsonl` 里一条失败都没有——事后完全查不出「是哪几个账号、为什么失败」。

mod support;

use ai_manager::app::accounts::{repo, AccountIdentity};
use ai_manager::app::checkin::Service;
use ai_manager::app::db::Db;
use ai_manager::app::log::{self, Level, LogFilter};
use ai_manager::app::net::MockHttp;
use std::sync::Arc;
use support::*;

const STATUS_URL: &str = "https://api.trae.cn/trae/api/v2/ug/checkin_credits/status";
const USAGE_URL: &str = "https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage";

fn seed(db: &Db, iid: &str, name: &str, with_secret: bool) {
    repo::upsert_identity(
        db,
        &AccountIdentity {
            iid: iid.into(),
            platform: "trae".into(),
            platform_label: "Trae CN".into(),
            account_id: iid.split(':').nth(1).unwrap_or(iid).into(),
            name: name.into(),
            expires_at: None,
            source: "storage.json".into(),
            requires_checkin: true,
            has_secret: true,
        },
        0,
    )
    .unwrap();
    if with_secret {
        let secret = br#"{"token":"jwt-live","expires_at":"","meta":{"uid":"u1"}}"#;
        repo::set_secret(db, iid, secret, None).unwrap();
    }
}

#[tokio::test]
async fn credits_failures_are_logged_per_account() {
    let paths = TestPaths::new();
    log::init_at(Level::Debug, &paths.join("logs")).unwrap();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let ok_iid = "trae:u1".to_string();
    let bad_iid = "trae:u2".to_string();
    seed(&db, &ok_iid, "正常账号", true);
    // 故意不写凭据：查询会在 load() 这一步失败，界面会显示「查询失败」
    seed(&db, &bad_iid, "无凭据账号", false);

    let http = Arc::new(MockHttp::new());
    http.on(
        STATUS_URL,
        200,
        r#"{"checked_in":false,"enable":true,"credits":10}"#,
    );
    http.on(
        USAGE_URL,
        200,
        r#"{"usage_summary":{"total_amount":100,"consumed_amount":40}}"#,
    );
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http, clock);

    let out = service
        .credits_batch(&[ok_iid.clone(), bad_iid.clone()], "user")
        .await;
    assert_eq!(out.len(), 2);
    assert!(
        out.iter()
            .find(|(iid, _)| iid == &bad_iid)
            .unwrap()
            .1
            .error
            .is_some(),
        "前提：这次刷新确实有一个账号查询失败"
    );

    let rows = log::query(&LogFilter {
        action_only: Some(true),
        ..Default::default()
    })
    .unwrap();

    let failures: Vec<_> = rows
        .iter()
        .filter(|line| line.action.as_deref() == Some("刷新积分"))
        .collect();
    assert_eq!(failures.len(), 1, "每个失败账号必须恰好一条失败记录");
    let row = failures[0];
    assert_eq!(
        row.subject.as_deref(),
        Some("无凭据账号"),
        "要能看出是哪个账号"
    );
    assert_eq!(row.outcome.as_deref(), Some("fail"));
    assert_eq!(row.level, Level::Warn, "账号级失败要能在异常筛选里看到");
    let evidence = row.evidence.as_deref().unwrap_or_default();
    assert!(
        evidence.contains("kind=") && evidence.contains("没有保存凭据"),
        "证据必须含 kind + 原文：{evidence}"
    );

    let summary = rows
        .iter()
        .find(|line| line.action.as_deref() == Some("checkin_credits"))
        .expect("批次收尾也要有一条汇总");
    assert_eq!(summary.ok, Some(false));
    assert!(
        summary.message.contains("总计=2") && summary.message.contains("失败=1"),
        "汇总要能让界面上的「1 个查询失败」对上账：{}",
        summary.message
    );
}

/// 触发方必须原样进批次汇总：总览页积分过期的自动补刷传 `auto`，
/// 日志里就不能标「手动」——否则排障时看不出这轮是谁触发的
/// （v1.9.1 九类测试发现：自动补刷曾与手动按钮共用 `user`）。
#[tokio::test]
async fn credits_batch_logs_client_trigger_verbatim() {
    let paths = TestPaths::new();
    log::init_at(Level::Debug, &paths.join("logs")).unwrap();
    let db = Arc::new(Db::open_at(&paths.join("app.db")).unwrap());
    let iid = "trae:u1".to_string();
    seed(&db, &iid, "正常账号", true);

    let http = Arc::new(MockHttp::new());
    http.on(
        STATUS_URL,
        200,
        r#"{"checked_in":false,"enable":true,"credits":10}"#,
    );
    http.on(
        USAGE_URL,
        200,
        r#"{"usage_summary":{"total_amount":100,"consumed_amount":40}}"#,
    );
    let clock = Arc::new(clock_at("2026-10-30T10:00:00+08:00"));
    let service = Service::new(db.clone(), http, clock);

    service.credits_batch(&[iid], "auto").await;

    let rows = log::query(&LogFilter {
        action_only: Some(true),
        ..Default::default()
    })
    .unwrap();
    let summary = rows
        .iter()
        .find(|line| line.action.as_deref() == Some("checkin_credits"))
        .expect("批次收尾汇总必须存在");
    assert_eq!(
        summary.trigger.as_deref(),
        Some("auto"),
        "触发方要按客户端传的原样记录"
    );
}
