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
    use ai_manager::app::task::CancelToken;

    let db = Arc::new(Db::open().expect("打开真实库"));
    let service = Service::new(db.clone(), Roots::system());
    let started = std::time::Instant::now();
    let report = service
        .scan(Arc::new(CancelToken::new()))
        .await
        .expect("扫描会话");
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

/// 整库快照的真机核对：备份出来的库能不能打开、行数对不对。
#[tokio::test]
#[ignore = "会读写真实数据目录下的 backups/，手动执行"]
async fn probe_backup() {
    use ai_manager::app::clients::Roots;
    use ai_manager::app::sessions::Service;

    let db = Arc::new(Db::open().expect("打开真实库"));
    let service = Service::new(db, Roots::system());
    let snapshot = service.backup().await.expect("备份");
    println!("\n{}", snapshot.summary());
    for source in &snapshot.sources {
        let path = std::path::Path::new(&snapshot.dir)
            .join("db")
            .join(format!("{source}.db"));
        let conn = rusqlite::Connection::open(&path).expect("打开备份库");
        let table = if source.starts_with("qoder") {
            "chat_sessions"
        } else {
            "sessions"
        };
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| {
                r.get(0)
            })
            .unwrap_or(-1);
        println!("  {source:<10} {:<12} {count} 行", table);
        assert!(count > 0, "{source} 的备份是空库");
    }
    assert!(!snapshot.sources.is_empty());
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
