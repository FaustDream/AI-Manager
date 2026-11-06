//! 真机探针：拿**真实账号库 + 真实网络**跑一遍「查状态 / 查积分」。
//!
//! 这里是**只读**的——只调查询接口，绝不调签到接口，跑多少次都不会领到东西。
//! 默认 `#[ignore]`，需要时手动执行：
//!
//! ```text
//! cargo test --test live_probe -- --ignored --nocapture
//! ```
//!
//! 用途：平台接口改字段时，这是第一个能看见真实返回长什么样的地方；
//! 也是 P3 验收里「配额三种来源显示正确」的实测凭据。
//!
//! 输出里**不打印任何令牌**，只有平台、脱敏昵称、余额、配额、下次可签。

use ai_manager::app::accounts::repo;
use ai_manager::app::checkin::Service;
use ai_manager::app::db::Db;
use ai_manager::app::net::RealHttp;
use ai_manager::SystemClock;
use std::sync::Arc;

#[tokio::test]
#[ignore = "会真的打平台接口，手动执行"]
async fn probe_every_account() {
    let db = Arc::new(Db::open().expect("打开真实账号库"));
    let accounts = repo::list(&db).expect("读账号表");
    println!("\n数据库里 {} 个账号", accounts.len());
    let service = Service::new(db.clone(), Arc::new(RealHttp::new()), Arc::new(SystemClock));

    let mut failed = Vec::new();
    for a in &accounts {
        if !a.has_secret {
            println!("- {:<18} {}｜待登录，跳过", a.platform, a.display_name());
            continue;
        }
        if a.requires_checkin {
            match service.status(&a.iid).await {
                Ok(s) => println!(
                    "- {:<18} {}｜余额 {}｜已签 {:?}｜下次 {}｜{}",
                    a.platform,
                    a.display_name(),
                    money(s.balance),
                    s.checked_in,
                    s.next_at
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_else(|| "—".into()),
                    s.summary
                ),
                Err(e) => {
                    println!(
                        "- {:<18} {}｜失败：{}",
                        a.platform,
                        a.display_name(),
                        e.message
                    );
                    failed.push(format!("{}：{}", a.platform, e.message));
                }
            }
        } else {
            match service.credits(&a.iid).await {
                Ok(c) => println!(
                    "- {:<18} {}｜余额 {}｜{}",
                    a.platform,
                    a.display_name(),
                    money(c.balance),
                    c.summary
                ),
                Err(e) => {
                    println!(
                        "- {:<18} {}｜失败：{}",
                        a.platform,
                        a.display_name(),
                        e.message
                    );
                    failed.push(format!("{}：{}", a.platform, e.message));
                }
            }
        }
    }

    println!("\n失败 {} 个 / 账号 {} 个", failed.len(), accounts.len());
    for line in &failed {
        println!("  × {line}");
    }
    // 全挂说明不是单个平台的问题（网络/证书/本机时间），值得红一次
    assert!(
        failed.len() < accounts.len(),
        "所有账号都查不到，先怀疑网络、系统时间或证书"
    );
}

/// 纯网络探针：几个**国际版**接口在当前网络（含系统代理）下是否可达。
/// 不需要账号库、不解密任何凭据，只回答「是不是连不上」。
///
/// ```text
/// cargo test --test live_probe probe_network -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "打真实网络，手动执行"]
async fn probe_network() {
    use ai_manager::app::net::Http;
    let http = RealHttp::new();
    for url in [
        "https://openapi.qoder.sh/api/v2/quota/usage",
        "https://qoder.com/",
        "https://www.workbuddy.ai/",
        "https://openapi.qoder.com.cn/",
    ] {
        match http.get(url).await {
            Ok(resp) => println!("- {url} → HTTP {}", resp.status),
            Err(e) => println!("- {url} → 失败：{}", e.message),
        }
    }
}

/// ZCode 领取活动探针：只读 `billing/preview` 与 `marketing/touch`，
/// 打印每个 ZCode 账号解析出的套餐（不领取、不碰验证码）。
///
/// ```text
/// cargo test --test live_probe probe_zcode_plans -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "打真实接口，手动执行"]
async fn probe_zcode_plans() {
    let db = Arc::new(Db::open().expect("打开真实账号库"));
    let accounts = repo::list(&db).expect("读账号表");
    let service = Service::new(db.clone(), Arc::new(RealHttp::new()), Arc::new(SystemClock));
    let mut zcode = 0;
    for a in accounts.iter().filter(|a| a.platform == "zcode") {
        zcode += 1;
        match service.zcode_claimable(&a.iid).await {
            Ok(plans) => {
                println!("- {} → {} 个可领套餐", a.display_name(), plans.len());
                for p in &plans {
                    println!(
                        "    {} | {} | {} | 生效 {}",
                        p.plan_id,
                        p.name,
                        if p.grants.is_empty() {
                            "（无 Token 明细）".to_string()
                        } else {
                            p.grants.join("；")
                        },
                        p.effective_at
                    );
                }
            }
            Err(e) => println!("- {} → 失败：{}", a.display_name(), e.message),
        }
    }
    if zcode == 0 {
        println!("库里没有 ZCode 账号");
    }
}

/// ZCode 活动接口**原始结构**探针：打印 `billing/preview` 的 JSON 片段。
/// 列表为空时先跑这个——平台换字段和「真没活动」在解析后长得一模一样。
///
/// ```text
/// cargo test --test live_probe probe_zcode_preview -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "打真实接口，手动执行"]
async fn probe_zcode_preview() {
    use ai_manager::app::accounts::credential::Credential;
    use ai_manager::app::accounts::repo as acc_repo;
    use ai_manager::app::providers::zclaim;

    let db = Arc::new(Db::open().expect("打开真实账号库"));
    let accounts = repo::list(&db).expect("读账号表");
    let http = RealHttp::new();
    for a in accounts.iter().filter(|a| a.platform == "zcode") {
        let Some(bytes) = acc_repo::secret_of(&db, &a.iid).expect("读凭据") else {
            println!("- {}：没有凭据", a.display_name());
            continue;
        };
        let cred = Credential::parse(&bytes).expect("解析凭据包");
        // 活动按客户端版本分级下发：官方能领、这里为空时，先排除「版本太低」
        for version in ["3.16.3"] {
            match zclaim::preview_payload_with(&cred, &http, None, version).await {
                Ok(data) => {
                    let text = data.to_string();
                    let shown: String = text.chars().take(600).collect();
                    println!(
                        "- {} preview v{version}：{} 字符\n  {shown}",
                        a.display_name(),
                        text.chars().count()
                    );
                }
                Err(e) => println!(
                    "- {} preview v{version}：失败 {}",
                    a.display_name(),
                    e.message
                ),
            }
            // 接口限流很紧，换通道前先停一下
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            match zclaim::marketing_payload(&cred, &http, None, 0).await {
                Ok(data) => {
                    let text = data.to_string();
                    let shown: String = text.chars().take(1200).collect();
                    println!(
                        "- {} marketing seq=0：{} 字符\n  {shown}",
                        a.display_name(),
                        text.chars().count()
                    );
                }
                Err(e) => println!("- {} marketing：失败 {}", a.display_name(), e.message),
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
}

/// 猫猫旅行状态探针：打印每个 WorkBuddy 国内版账号的**实时**旅行状态。
/// 定时任务「什么都不做」时不留日志，只能靠这个看它当时到底卡在哪个状态。
///
/// ```text
/// cargo test --test live_probe probe_travel -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "打真实接口，手动执行"]
async fn probe_travel() {
    use ai_manager::app::accounts::credential::Credential;
    use ai_manager::app::accounts::repo as acc_repo;
    use ai_manager::app::providers;

    let db = Arc::new(Db::open().expect("打开真实账号库"));
    let accounts = repo::list(&db).expect("读账号表");
    let http = RealHttp::new();
    for a in accounts.iter().filter(|a| a.platform == "workbuddy") {
        if a.iid.ends_with("@intl") {
            println!("- {}：国际版没有猫猫旅行，跳过", a.display_name());
            continue;
        }
        let Some(bytes) = acc_repo::secret_of(&db, &a.iid).expect("读凭据") else {
            println!("- {}：没有凭据", a.display_name());
            continue;
        };
        let cred = Credential::parse(&bytes).expect("解析凭据");
        let Some(provider) = providers::for_iid(&a.iid) else {
            println!("- {}：没有对应 provider", a.display_name());
            continue;
        };
        match provider.travel_status(&cred, &http).await {
            Ok(s) => println!(
                "- {}：state={:?} limit={} reward={:?} arrive_at={:?}",
                a.display_name(),
                s.state,
                s.daily_limit_reached,
                s.reward_credit,
                s.arrive_at
            ),
            Err(e) => println!("- {}：失败 {}", a.display_name(), e.message),
        }
    }
}

/// 成长中心探针：打印任务列表、活跃地图、连续天数（全部只读）。
/// 用来确认「每日活跃」到底由哪几个 task code 判定，不靠猜。
///
/// ```text
/// cargo test --test live_probe probe_growth -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "打真实接口，手动执行"]
async fn probe_growth() {
    use ai_manager::app::accounts::credential::Credential;
    use ai_manager::app::accounts::repo as acc_repo;
    use ai_manager::app::providers::workbuddy::WorkBuddyProvider;

    let db = Arc::new(Db::open().expect("打开真实账号库"));
    let accounts = repo::list(&db).expect("读账号表");
    let http = RealHttp::new();
    let provider = WorkBuddyProvider::new(false);
    let account = accounts
        .iter()
        .find(|a| a.platform == "workbuddy" && !a.iid.ends_with("@intl") && a.has_secret)
        .expect("至少一个国内版账号");
    let bytes = acc_repo::secret_of(&db, &account.iid)
        .expect("读凭据")
        .expect("有凭据");
    let cred = Credential::parse(&bytes).expect("解析凭据包");

    match provider.growth_tasks(&cred, &http).await {
        Ok(data) => {
            let tasks = data
                .pointer("/data/tasks")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            println!("tasks 共 {} 条", tasks.len());
            for t in &tasks {
                println!(
                    "  {:<26} {:<8} {:<9} {}/{}  {}",
                    t.get("task_code").and_then(|v| v.as_str()).unwrap_or("?"),
                    t.get("task_type").and_then(|v| v.as_str()).unwrap_or("?"),
                    t.get("accept_status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                    t.pointer("/progress/current")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    t.pointer("/progress/target")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                    t.get("title").and_then(|v| v.as_str()).unwrap_or("")
                );
            }
        }
        Err(e) => println!("tasks 失败：{}", e.message),
    }
    match provider.growth_streak(&cred, &http).await {
        Ok(data) => println!("streak = {data}"),
        Err(e) => println!("streak 失败：{}", e.message),
    }
    match provider.growth_heatmap(&cred, &http, "2026-11").await {
        Ok(data) => {
            let cells = data
                .pointer("/data/cells")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            println!("heatmap cells = {}", cells.len());
            for c in cells.iter() {
                let date = c.get("date").and_then(|v| v.as_str()).unwrap_or("");
                if date.starts_with("2026-11") {
                    println!(
                        "  {date} score={} newBuddy={}",
                        c.get("score").and_then(|v| v.as_i64()).unwrap_or(0),
                        c.get("has_new_buddy")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    );
                }
            }
        }
        Err(e) => println!("heatmap 失败：{}", e.message),
    }
    match provider.core_call(&cred, &http, "/v1/config", None).await {
        Ok(data) => {
            let text = data.to_string();
            println!(
                "config = {} chars\n{}",
                text.chars().count(),
                text.chars().take(2000).collect::<String>()
            );
        }
        Err(e) => println!("config 失败：{}", e.message),
    }
    // `hy4-free` 是客户端当前在用的免费模型（来自 projects/*.jsonl 的 providerData）
    let body = serde_json::json!({
        "model": "hy4free",
        "messages": [{ "role": "user", "content": "你好" }],
        "stream": true
    });
    match provider
        .core_text(&cred, &http, "/v2/chat/completions", Some(&body))
        .await
    {
        Ok(text) => println!(
            "chat ok = {} chars\n{}",
            text.chars().count(),
            text.chars().take(400).collect::<String>()
        ),
        Err(e) => println!("chat 失败：{}", e.message),
    }
}

/// 套餐与积分接口探针：把候选路径逐个打一遍，看哪个返回积分明细。
/// 路径候选来自「页面 bundle 里找不到就按业务命名穷举」，命中后回填常量。
///
/// ```text
/// cargo test --test live_probe probe_plans_usage -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "打真实接口，手动执行"]
async fn probe_plans_usage() {
    use ai_manager::app::accounts::credential::Credential;
    use ai_manager::app::accounts::repo as acc_repo;
    use ai_manager::app::providers::workbuddy::WorkBuddyProvider;

    let db = Arc::new(Db::open().expect("打开真实账号库"));
    let accounts = repo::list(&db).expect("读账号表");
    let http = RealHttp::new();
    let provider = WorkBuddyProvider::new(false);
    let account = accounts
        .iter()
        .find(|a| a.platform == "workbuddy" && !a.iid.ends_with("@intl") && a.has_secret)
        .expect("至少一个国内版账号");
    let bytes = acc_repo::secret_of(&db, &account.iid)
        .expect("读凭据")
        .expect("有凭据");
    let cred = Credential::parse(&bytes).expect("解析凭据包");

    // 对比「空 body」与「正式代码用的筛选 body」，看 Status 过滤掉了多少资源包
    for (tag, body) in [
        ("empty", serde_json::json!({})),
        (
            "paged",
            serde_json::json!({"PageNumber":1,"PageSize":200,"ProductCode":"p_tcaca","Status":[0,3]}),
        ),
    ] {
        match provider
            .core_text(
                &cred,
                &http,
                "/v2/billing/meter/get-user-resource",
                Some(&body),
            )
            .await
        {
            Ok(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                let data = value
                    .pointer("/data/Response/Data")
                    .cloned()
                    .unwrap_or_default();
                let items = data
                    .get("Accounts")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                let positive = items
                    .iter()
                    .filter(|item| {
                        item.get("CycleCapacityRemain")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0)
                            > 0.0
                    })
                    .count();
                println!(
                    "{tag}: TotalCount={:?} TotalDosage={:?} accounts={} remain>0={}",
                    data.get("TotalCount")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    data.get("TotalDosage")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    items.len(),
                    positive
                );
                if let Some(first) = items.first() {
                    println!(
                        "  first keys = {:?}",
                        first
                            .as_object()
                            .map(|o| o.keys().cloned().collect::<Vec<_>>())
                    );
                }
            }
            Err(e) => println!("{tag} → {}", e.message),
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
}

fn money(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "—".into())
}

/// 会话索引的真机核对：各来源条数 + 前几条长什么样。
/// 和上面的签到探针一样只读客户端库，产物落到本机 `data/app.db` 的 `session_index`。
#[tokio::test]
#[ignore = "会扫真实客户端目录，手动执行"]
async fn probe_sessions() {
    use ai_manager::app::clients::Roots;
    use ai_manager::app::sessions::Service;

    let db = Arc::new(Db::open().expect("打开真实库"));
    let service = Service::new(db.clone(), Roots::system());
    let started = std::time::Instant::now();
    let report = service.scan().await.expect("扫描会话");
    println!(
        "\n{}（{:.1}s）",
        report.summary(),
        started.elapsed().as_secs_f64()
    );
    for error in &report.errors {
        println!("  ! {error}");
    }

    let facets = service.facets().await.expect("读筛选");
    println!("\n来源分布（共 {} 条）：", facets.total);
    for item in &facets.sources {
        println!("  {:<22} {:>4}", item.label, item.count);
    }

    let page = service
        .page(ai_manager::app::sessions::Filter::default(), None, 5)
        .await
        .expect("读首页");
    println!("\n最近 5 条：");
    for row in &page.items {
        println!(
            "  {:<20} {:>6} 条 {:>8} B  {:?}  {}",
            row.title.chars().take(20).collect::<String>(),
            row.msgs.unwrap_or(0),
            row.size.unwrap_or(0),
            row.state,
            row.source_label
        );
    }
    assert!(page.total > 0, "一条会话都没扫到，先怀疑路径");

    // 正文预览：三种来源各挑一条，验它们真的能读出人来
    let all = service
        .page(ai_manager::app::sessions::Filter::default(), None, 500)
        .await
        .expect("读全量用于挑样本");
    for target in ["workbuddy", "codebuddy", "qoder", "zcode", "trae"] {
        let Some(row) = all
            .items
            .iter()
            .find(|r| r.kind == target && (target == "trae" || r.has_body))
        else {
            continue;
        };
        println!(
            "  [{target}] source={} account={} sid={} msgs={:?}",
            row.source_key, row.account, row.session_id, row.msgs
        );
        let key = format!(
            "{}|{}|{}|{}",
            row.kind, row.source_key, row.account, row.session_id
        );
        let preview = service.preview(key).await.expect("预览");
        let first = preview.messages.first().map(|m| {
            let text: String = m.text.chars().take(40).collect();
            format!("[{}] {}", m.role, text.replace('\n', " "))
        });
        println!(
            "  {:<10} 共 {:>3} 条（显示 {:>2}，省略 {:>3}）{}  {}",
            target,
            preview.total,
            preview.messages.len(),
            preview.dropped,
            if preview.note.is_empty() {
                String::new()
            } else {
                format!("｜{}", preview.note)
            },
            first.unwrap_or_else(|| "（无正文）".into())
        );
    }
}

/// 猫猫旅行协议探针：只发 GET，不领取、不派出。
///
/// wb_tool 的端点来自静态分析（方法/参数未知），所以先用真实账号把
/// `info / config / status` 三个只读端点跑一遍，确认域名、鉴权头与返回结构。
/// 输出会把 token 原文替换成 `***`，只保留响应状态与前 700 字。
#[tokio::test]
#[ignore = "会真的打 WorkBuddy 活动接口，手动执行"]
async fn probe_workbuddy_activity() {
    use ai_manager::app::accounts::credential::Credential;
    use ai_manager::app::net::{Http, HttpRequest};

    let db = Arc::new(Db::open().expect("打开真实账号库"));
    let accounts = repo::list(&db).expect("读账号表");
    let Some(account) = accounts
        .into_iter()
        .find(|a| a.platform == "workbuddy" && !a.iid.ends_with("@intl") && a.has_secret)
    else {
        println!("没有国内版 WorkBuddy 账号，跳过");
        return;
    };
    let bytes = repo::secret_of(&db, &account.iid)
        .expect("读凭据")
        .expect("没有密文");
    let cred = Credential::parse(&bytes).expect("解析凭据");
    let http = RealHttp::new();

    let bases = [
        "https://www.workbuddy.cn",
        "https://www.workbuddy.cn/api",
        "https://copilot.tencent.com",
        "https://copilot.tencent.com/v2",
    ];
    let paths = [
        "/activity/growth/buddy/info",
        "/activity/growth/buddy/travel/config",
        "/activity/growth/buddy/travel/status",
        "/activity/growth/buddy/travel/records",
    ];

    println!("\n账号：{}", account.display_name());
    for base in bases {
        for path in paths {
            let url = format!("{base}{path}");
            let req = HttpRequest::get(&url)
                .header("Authorization", format!("Bearer {}", cred.token))
                .header("X-User-Id", cred.meta_str("uid").unwrap_or(""))
                .header("X-Domain", cred.meta_str("domain").unwrap_or(""))
                .header("User-Agent", cred.user_agent("Mozilla/5.0 CodeBuddy/5.6.0"))
                .header("Accept", "application/json");
            match http.send(req).await {
                Ok(resp) => {
                    let body = redact(&resp.body, &cred.token);
                    println!("\n{url}\n  HTTP {}  {}", resp.status, resp.is_ok());
                    if path.ends_with("/records") {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                            let records = value
                                .pointer("/data/records")
                                .and_then(|v| v.as_array())
                                .cloned()
                                .unwrap_or_default();
                            println!("  记录 {} 条", records.len());
                            for record in records.iter().take(2) {
                                let mut slim = record.clone();
                                if let Some(obj) = slim.as_object_mut() {
                                    obj.remove("location");
                                }
                                println!(
                                    "  {}",
                                    truncate(
                                        &serde_json::to_string(&slim).unwrap_or_default(),
                                        1200
                                    )
                                );
                                if let Some(location) = record.get("location") {
                                    println!(
                                        "  location: id={:?} code={:?} name={:?}",
                                        location.get("id"),
                                        location.get("code"),
                                        location.get("name")
                                    );
                                }
                            }
                        }
                    } else {
                        println!("  {}", truncate(&body, 700));
                    }
                }
                Err(e) => println!("\n{url}\n  请求失败：{}", e.message),
            }
        }
    }
}

/// 旅行卡片的服务层真机只读核对：走界面同一条 `activity_overview`。
#[tokio::test]
#[ignore = "会真的打 WorkBuddy 活动接口，手动执行"]
async fn probe_activity_overview() {
    use ai_manager::app::activity::Service;

    let db = Arc::new(Db::open().expect("打开真实账号库"));
    let accounts = repo::list(&db).expect("读账号表");
    let Some(account) = accounts
        .into_iter()
        .find(|a| a.platform == "workbuddy" && !a.iid.ends_with("@intl") && a.has_secret)
    else {
        println!("没有国内版 WorkBuddy 账号，跳过");
        return;
    };
    let service = Service::new(db, Arc::new(RealHttp::new()));
    let overview = service.overview(&account.iid).await.expect("读取旅行概览");
    println!(
        "\n账号：{}\n状态：{:?} · 地点 {:?} · 回来 {:?} · 奖励 {:?}\nBuddy：{:?}\n地点数：{}\n记录数：{}\n偏好：{:?}\n警告：{:?}",
        account.display_name(),
        overview.status.state,
        overview.status.location.map(|l| l.name),
        overview.status.arrive_at,
        overview.status.reward_credit,
        overview.status.buddy_name,
        overview.config.locations.len(),
        overview.records.len(),
        overview.prefs,
        overview.warnings
    );
}

fn redact(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "***")
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!(
        "{}…（共 {} 字）",
        text.chars().take(max).collect::<String>(),
        text.chars().count()
    )
}

/// 技能与 MCP 的真机只读核对：库里有什么、各平台配置里有什么。
/// 只打印名字与命令，**不打印 env**（里面可能有令牌）。
#[tokio::test]
#[ignore = "读真实客户端配置，手动执行"]
async fn probe_hub() {
    use ai_manager::app::hub::{self, Hub};

    let hub = Hub::system();
    let snapshot = hub::snapshot(&hub);
    println!(
        "\n技能 {} 个 · MCP {} 条 · 平台 {}",
        snapshot.skills.len(),
        snapshot.mcp.len(),
        snapshot.targets.len()
    );
    for row in &snapshot.targets {
        println!(
            "  {:<26} 技能 {:>2} · MCP {:>2}  {}",
            row.label, row.skills, row.mcp, row.mcp_file
        );
    }
    for skill in &snapshot.skills {
        let deployed: Vec<&str> = skill
            .states
            .iter()
            .filter(|(_, state)| *state == "synced")
            .map(|(key, _)| key.as_str())
            .collect();
        println!(
            "  技能 {:<24} {:<6} 已部署：{}",
            skill.name,
            if skill.valid { "有效" } else { "无效" },
            deployed.join(",")
        );
    }
    for item in &snapshot.mcp {
        println!(
            "  MCP  {:<24} 命令 {:<12} 已写入 {} {}",
            item.name,
            item.command,
            item.targets.join(","),
            if item.foreign {
                "（含别人的同名条目）"
            } else {
                ""
            }
        );
    }
}
