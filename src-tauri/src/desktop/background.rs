//! 后台常驻任务：调度器、启动扫描、用量采集、活动轮询、事件桥。
//!
//! 全部是「启动后自转」的 spawn 点，只负责把业务服务按节奏叫起来；
//! 规则与业务都在 `app/` 对应模块。新增后台任务加在这里，
//! `mod.rs` 的 setup 只保留组装。

use crate::app::db::Db;
use crate::app::log::{self, Level};
use std::sync::Arc;

/// 定时任务调度器：睡到下次执行点，配置变更即时唤醒。
///
/// **定时器必须在 Rust 侧**：前端 `setInterval` 在窗口隐藏/被挂起时就停了，
/// 而后台常驻正是这个工具的主场景。
///
/// 规则本身是用户配的（`app::schedule`，存 kv），这里只负责「到点了就叫起来」，
/// 并且每次触发都落一条 `trigger=schedule` 的执行记录——用户事后能看到
/// 「它自己跑过什么、成没成、花了多久」。
pub fn spawn_scheduler(
    db: Arc<Db>,
    service: Arc<crate::app::checkin::Service>,
    activity: Arc<crate::app::activity::Service>,
) {
    use std::collections::HashSet;
    use std::sync::Mutex;

    tauri::async_runtime::spawn(async move {
        // 等首屏稳定再开始判点，别和账号列表抢 SQLite 的读连接
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(20)) => {},
            _ = crate::app::schedule::changed() => {},
        }
        // 正在跑的 id：一轮签到几十秒，期间不能被下一跳重复捞起来
        let running: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        loop {
            let now = chrono::Local::now();
            let list = crate::app::runtime::blocking({
                let db = db.clone();
                move || crate::app::schedule::list(&db)
            })
            .await
            .ok()
            .and_then(|result| result.ok())
            .unwrap_or_default();

            let mut next_at: Option<chrono::DateTime<chrono::Local>> = None;
            for item in &list {
                // 计算下次执行时刻时，把「本轮就要跑」的任务按已跑过处理，
                // 否则 every 规则会拿着旧的 last_run 算出一个过去时刻，导致 5 秒空转
                let mut candidate = item.clone();
                if item.due(now) {
                    candidate.last_run = Some(now.to_rfc3339());
                }
                if let Some(next) = candidate.next_run(now) {
                    next_at = Some(match next_at {
                        Some(current) => current.min(next),
                        None => next,
                    });
                }
                if !item.due(now) {
                    continue;
                }
                {
                    let mut guard = running.lock().unwrap_or_else(|e| e.into_inner());
                    if !guard.insert(item.id.clone()) {
                        continue;
                    }
                }
                let service = service.clone();
                let activity = activity.clone();
                let running = running.clone();
                let db = db.clone();
                let id = item.id.clone();
                tauri::async_runtime::spawn(async move {
                    let fresh = crate::app::runtime::blocking({
                        let id = id.clone();
                        move || crate::app::schedule::take_due(&db, &id, chrono::Local::now())
                    })
                    .await;
                    match fresh {
                        Ok(Ok(Some(item))) => run_schedule(&service, &activity, &item).await,
                        Ok(Ok(None)) => {}
                        Ok(Err(e)) | Err(e) => log::fail(
                            "schedule",
                            "schedule_start",
                            "读取或标记定时任务失败，未执行",
                            e.evidence(),
                        ),
                    }
                    running
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&id);
                });
            }

            // 睡到最近一条规则的下次执行时刻；最长 30 分钟兜底，防止时钟跳变漏事件
            let wait_ms = next_at
                .map(|next| (next - now).num_milliseconds())
                .unwrap_or(30 * 60 * 1000)
                .clamp(5_000, 30 * 60 * 1000) as u64;
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(wait_ms)) => {},
                _ = crate::app::schedule::changed() => {},
            }
        }
    });
}

/// 执行一条定时任务的动作本体：返回 (是否成功, 结果文案)，**不写日志**。
///
/// 定时触发和「立即执行」都走这里，只是外面各自记一条 trigger 不同的执行记录——
/// 两条链路各写一遍动作分支，早晚会漂。
/// 一次定时执行的结论。
///
/// `changed` 是这次优化新加的判断依据：**没产生任何变化的轮询不写日志**。
/// 积分刷新与旅行轮询都是「按用户配的间隔」在跑（默认积分 5 分钟、旅行 1 小时），
/// 全绿时逐条记录只是心跳噪声；「定时任务到底跑没跑」由「定时任务」页的 last_run 回答，
/// 日志只留变化与失败。
pub(crate) struct ScheduleOutcome {
    pub ok: bool,
    pub detail: String,
    pub changed: bool,
}

pub(crate) async fn execute_schedule(
    service: &Arc<crate::app::checkin::Service>,
    activity: &Arc<crate::app::activity::Service>,
    item: &crate::app::schedule::Schedule,
) -> ScheduleOutcome {
    use crate::app::events;
    use crate::app::schedule::ScheduleAction::*;

    match item.action {
        CheckinRound => {
            let round = service.run_round("schedule").await;
            // 界面收到就更新底部那一行；收不到也不影响结算
            events::emit("checkin:round", &round);
            ScheduleOutcome {
                ok: round.failed == 0,
                detail: round.detail(),
                changed: round.success > 0 || round.failed > 0,
            }
        }
        CreditsRound => {
            let (total, failed) = service.credits_round().await;
            if total > 0 {
                events::emit("credits:round", total);
            }
            ScheduleOutcome {
                ok: failed == 0,
                detail: format!("{total} 个账号，失败 {failed}"),
                // 全绿没有可看的信息：余额数字在账号列表上，日志里不必每 5 分钟重复一遍
                changed: failed > 0,
            }
        }
        TravelRound => {
            let round = match item.account_iid.as_deref() {
                Some(iid) => activity.round_for("schedule", Some(iid)).await,
                None => activity.round("schedule").await,
            };
            events::emit("activity:round", &round.message);
            ScheduleOutcome {
                ok: round.failed == 0,
                detail: round.message,
                changed: round.claimed > 0 || round.departed > 0 || round.failed > 0,
            }
        }
    }
}

/// 定时触发：执行 + 只在「有变化」时写一条执行记录。
async fn run_schedule(
    service: &Arc<crate::app::checkin::Service>,
    activity: &Arc<crate::app::activity::Service>,
    item: &crate::app::schedule::Schedule,
) {
    let started = std::time::Instant::now();
    let outcome = execute_schedule(service, activity, item).await;
    // 无事可做的心跳轮询不留痕：这类「正常流程」每天能刷出上百行，
    // 真正需要排查时反而把失败记录淹了。服务内部的动作记录也已按同一口径收敛。
    if outcome.changed {
        log::action(
            &format!("schedule.{}", item.id),
            "schedule",
            outcome.ok,
            started.elapsed().as_millis() as u64,
            format!(
                "定时「{}」（{}）：{}",
                item.name,
                item.rule_text(),
                outcome.detail
            ),
        );
    }
}

/// ZCode 活动自动领取（v1 行为，开关 kv `zcode_auto_claim` **默认关**）：
/// 每 15 分钟探测一轮，找到可领的就领第一个——领取会开浏览器过验证码，
/// 所以必须是用户主动打开的开关。
pub fn spawn_zcode_auto_claim(service: Arc<crate::app::checkin::Service>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
            let enabled = crate::app::config::get().activity.zcode_auto_claim;
            if !enabled {
                continue;
            }
            let started = std::time::Instant::now();
            if let Some(line) = service.zcode_auto_claim_round().await {
                log::action(
                    "schedule.zcode_claim",
                    "schedule",
                    true,
                    started.elapsed().as_millis() as u64,
                    format!("ZCode 自动领取：{line}"),
                );
                crate::app::events::emit("zcode:claim", &line);
            }
        }
    });
}

/// 会话索引增量扫描：启动 8 秒后一次，之后每 5 分钟一次。
///
/// 不引文件监听：客户端写 WAL 时一次会话会产生大量事件，反而把扫描频率放大；
/// 5 分钟轮询 + 手动刷新已经能覆盖「外部新增/删除后列表更新」，
/// 且不新增 `notify` 依赖。
pub fn spawn_session_scan(service: Arc<crate::app::sessions::Service>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        loop {
            let started = std::time::Instant::now();
            match service.scan().await {
                Ok(report) => {
                    if report.scanned > 0 || report.archived > 0 || !report.errors.is_empty() {
                        log::action(
                            "session_scan",
                            "schedule",
                            report.errors.is_empty(),
                            started.elapsed().as_millis() as u64,
                            report.summary(),
                        );
                    }
                    crate::app::events::emit("session:scan", &report);
                }
                Err(e) => log::fail("sessions", "session_scan", "定时会话扫描失败", e.evidence()),
            }
            tokio::time::sleep(std::time::Duration::from_secs(5 * 60)).await;
        }
    });
}

/// 本地用量采集：启动 15 秒后跑一次，之后每 30 分钟增量扫一遍。
///
/// 只读客户端落盘文件，慢一点没关系；失败只记日志，不影响任何业务。
pub fn spawn_usage_scan(db: Arc<Db>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        loop {
            let db = db.clone();
            match crate::app::runtime::blocking(move || {
                crate::app::usage::scan_all(&db, &crate::app::clients::Roots::system())
            })
            .await
            {
                Ok(Ok(report)) if report.inserted > 0 => log::log(
                    Level::Info,
                    "usage",
                    format!(
                        "用量采集：新增 {} 条（扫描 {}）",
                        report.inserted, report.scanned
                    ),
                ),
                Ok(Ok(_)) => {}
                Ok(Err(e)) | Err(e) => {
                    log::fail("usage", "usage_scan", "用量采集失败", e.evidence())
                }
            }
            // 间隔每轮都读一次配置：设置界面改完，下一轮就按新值走
            let interval_min = crate::app::config::get().usage.scan_interval_min.max(1);
            tokio::time::sleep(std::time::Duration::from_secs(interval_min * 60)).await;
        }
    });
}

/// 事件桥（源侧）：把 lib 层的两个广播源接进统一事件总线
/// （日志实时行 → `log:line`；新领取记录 → `claim:recorded`）。
/// 桌面壳与网页壳共用，任何壳启动都要调它。
pub fn spawn_source_bridges() {
    tauri::async_runtime::spawn(async move {
        let mut rx = log::subscribe_live();
        loop {
            match rx.recv().await {
                Ok(line) => {
                    crate::app::events::emit("log:line", &line);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // 领取记录是本地台账：签到 / ZCode 活动 / 猫猫旅行任一路径写成功后推一条，
    // 详情面板据此立刻重取，不用等用户点刷新或下一轮轮询。
    tauri::async_runtime::spawn(async move {
        let mut rx = crate::app::checkin::store::subscribe_claims();
        loop {
            match rx.recv().await {
                Ok(iid) => {
                    crate::app::events::emit("claim:recorded", iid);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// 事件桥（WebView 侧，仅桌面壳）：把总线上的事件转发给前端。
/// 网页壳不调它——`desktop::web` 直接订阅总线走 SSE。
pub fn spawn_webview_bridge(app: tauri::AppHandle) {
    use tauri::Emitter;

    tauri::async_runtime::spawn(async move {
        let mut rx = crate::app::events::subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let _ = app.emit(&event.name, event.payload);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// 桌面壳的完整事件桥：源侧 + WebView 转发。
pub fn spawn_event_bridges(app: tauri::AppHandle) {
    spawn_source_bridges();
    spawn_webview_bridge(app);
}

/// 启动自动扫描账号：界面不再提供「扫描客户端」按钮，账号同步改为开机后台执行。
///
/// 延后 2 秒再跑：先让窗口上屏，扫描要读多个客户端目录，不该和冷启动抢 IO。
/// 增量扫描本身很快；失败只记日志，不阻塞窗口。
pub fn spawn_startup_scan(db: Arc<Db>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let started = std::time::Instant::now();
        match crate::app::runtime::blocking(move || crate::app::accounts::scan_all(&db)).await {
            Ok(Ok(report)) => {
                log::action(
                    "account_scan",
                    "startup",
                    true,
                    started.elapsed().as_millis() as u64,
                    format!(
                        "启动自动扫描：{} 个账号（新增 {}）",
                        report.scanned, report.added
                    ),
                );
                // 扫描完通知界面重拉列表与「当前登录」
                crate::app::events::emit("accounts:scanned", ());
            }
            Ok(Err(e)) => log::fail("accounts", "account_scan", "启动自动扫描失败", e.evidence()),
            Err(e) => log::fail(
                "accounts",
                "account_scan",
                "启动自动扫描失败",
                format!("kind=io · 原文={e}"),
            ),
        }
    });
}

/// 启动自动同步技能（STAGES §7.1）。开关在 kv `hub.auto_sync`，缺省视为开（对齐 v1）。
/// 挪到启动 20 秒后执行，别跟账号列表抢首屏；MCP 不参与（见 `skills::sync_all` 的注释）。
pub fn spawn_hub_auto_sync(db: Arc<Db>, hub: Arc<crate::app::hub::Hub>) {
    tauri::async_runtime::spawn(async move {
        let enabled = crate::app::runtime::blocking(move || {
            db.kv_get("hub.auto_sync")
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_str::<bool>(&v).ok())
                .unwrap_or(true)
        })
        .await;
        // 读开关失败按缺省开处理（对齐 v1）；panic 导致的 Err 也走这一支
        if !enabled.unwrap_or(true) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        let report = crate::app::runtime::blocking(move || crate::app::hub::skills::sync_all(&hub))
            .await
            .unwrap_or_default();
        if report.done() > 0 {
            log::log(
                Level::Info,
                "hub",
                format!("启动自动同步：{}", report.summary()),
            );
        }
    });
}
