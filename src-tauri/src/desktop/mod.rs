//! Tauri 外壳：窗口、托盘、单实例、IPC 命令。
//!
//! 这一层**只做转接**，不放业务逻辑——所有能力来自 `app/` 下的模块。
//! 命令的「类型」由 ts-rs 生成，「名单」由 `generate_handler!` 保证（架构 D8）。
//!
//! 两条 P1 就得立住的规矩：
//!   - 关窗口 = 隐藏，不退出；退出只走托盘菜单或 `app_quit`
//!   - 数据库调用一律 `spawn_blocking`，否则界面会在等结果期间变灰

use crate::app::db::Db;
use crate::app::error::AppError;
use crate::app::log::{self, Level};
use crate::app::process;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

pub struct AppState {
    pub db: Arc<Db>,
    /// 签到编排。持有网络与时钟，命令层只转接
    pub checkin: Arc<crate::app::checkin::Service>,
    /// 猫猫旅行编排（WorkBuddy 国内版）
    pub activity: Arc<crate::app::activity::Service>,
    /// 会话索引。只读写 app.db，客户端库由扫描任务自己碰
    pub sessions: Arc<crate::app::sessions::Service>,
    /// 技能与 MCP：只写配置文件，每次写前备份
    pub hub: Arc<crate::app::hub::Hub>,
    /// 单实例句柄必须活到进程结束，Drop 提前释放会让第二次启动挤进来
    _instance: Option<process::SingleInstance>,
}

impl AppState {
    fn db_path_label(&self) -> String {
        crate::app::paths::app_db().display().to_string()
    }
}

pub(crate) fn err_text(e: AppError) -> String {
    // 前端拿到的永远是 {kind, message} 的结构化错误
    serde_json::to_string(&e).unwrap_or(e.message)
}

pub mod commands;

// ------------------------------------------------------------------ 启动

/// 开发版（debug）必须连前端 dev server；连不上就**在进 WebView 之前**说清楚。
///
/// 不这么做的后果：WebView2 会直接渲染 Edge 的「无法访问此页面 / 127.0.0.1 拒绝连接」，
/// 用户看到的是一个浏览器错误页，而不是「你打开的是开发版」这句话。
#[cfg(debug_assertions)]
fn ensure_dev_server() -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let addr: SocketAddr = match "127.0.0.1:5273".parse() {
        Ok(addr) => addr,
        Err(_) => return true,
    };
    if TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok() {
        return true;
    }

    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
    // 用 concat! 而不是续行：续行会把下一行的缩进带进字符串，弹窗里就会莫名换行
    let text = HSTRING::from(concat!(
        "这是开发版（debug），需要先启动前端 dev server：\n\n",
        "    npm run dev\n\n",
        "日常使用请双击桌面的「AI Manager」快捷方式——它启动的是 release 版，",
        "前端资源已内嵌，不需要任何服务。",
    ));
    let caption = HSTRING::from("AI Manager · 开发版");
    unsafe {
        MessageBoxW(None, &text, &caption, MB_OK | MB_ICONWARNING);
    }
    false
}

pub fn run() -> tauri::Result<()> {
    #[cfg(debug_assertions)]
    if !ensure_dev_server() {
        return Ok(());
    }
    // 单实例必须在建窗口之前拿到，否则第二个实例会先闪一下窗口再退出
    let instance = match process::acquire_single_instance("AIManager.SingleInstance") {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => {
            // 已有实例在跑：把它的窗口叫到前面，然后自己退出
            focus_existing();
            return Ok(());
        }
        Err(e) => {
            // 拿不到互斥体不是小事：双开会并发写同一个 app.db，至少要留痕
            log::log(
                Level::Warn,
                "app",
                format!("单实例互斥体获取失败（{}），双开保护失效", e.message),
            );
            None
        }
    };

    tauri::Builder::default()
        .setup(move |app| {
            let state = init_state(instance)?;
            let service = state.checkin.clone();
            let activity = state.activity.clone();
            let hub = state.hub.clone();
            let scan_db = state.db.clone();
            let usage_db = state.db.clone();
            let sync_db = state.db.clone();
            let sched_db = state.db.clone();
            // 应用配置：加载 data/config.toml（首次会把旧 kv 配置键迁移进来）
            crate::app::config::init(&state.db);
            // 清掉「待处理告警」下线后留在 kv 里的死键（通知签名 / 静默表）：
            // 功能已整体移除，留着只会让后面查 kv 的人困惑。启动时顺手清一次。
            let dead_kv_db = state.db.clone();
            app.manage(state);

            build_tray(app)?;
            hook_window(app);
            std::thread::spawn(move || {
                for key in ["alert.notified", "alert.muted"] {
                    let _ = dead_kv_db.kv_delete(key);
                }
            });
            // 兜底：页面加载事件没来（异常）也要把窗口显示出来，否则用户看不到任何东西
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    if let Some(window) = handle.get_webview_window("main") {
                        if !window.is_visible().unwrap_or(true) {
                            let _ = window.show();
                        }
                    }
                });
            }
            spawn_scheduler(sched_db, service.clone(), activity, app.handle().clone());
            spawn_startup_scan(scan_db, app.handle().clone());
            spawn_hub_auto_sync(sync_db, hub);
            spawn_zcode_auto_claim(service, app.handle().clone());
            spawn_event_bridges(app.handle().clone());
            spawn_usage_scan(usage_db);

            // 启动本身也是一次「动作」：执行动作里能看到每次重启的时间点
            log::action("app.startup", "startup", true, 0, "外壳启动完成");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::shell::ui_ready,
            commands::shell::app_version,
            commands::shell::app_quit,
            commands::settings::settings_get,
            commands::settings::settings_set,
            commands::shell::app_show_window,
            commands::shell::open_external,
            commands::shell::open_private_browser,
            commands::accounts::account_list,
            commands::accounts::account_set_enabled,
            commands::accounts::account_set_alias,
            commands::accounts::account_set_tags,
            commands::accounts::account_set_note,
            commands::accounts::account_batch_diagnose,
            commands::accounts::account_export,
            commands::accounts::account_reward_credits,
            commands::accounts::account_reorder,
            commands::accounts::account_delete,
            commands::accounts::account_scan,
            commands::accounts::account_switch,
            commands::accounts::account_switch_codebuddy,
            commands::accounts::account_codebuddy_targets,
            commands::accounts::account_restore_sessions,
            commands::accounts::account_uid,
            commands::accounts::account_current,
            commands::accounts::account_add_prepare,
            commands::accounts::account_login_start,
            commands::accounts::account_relogin_start,
            commands::accounts::account_refresh_all,
            commands::accounts::account_open_client,
            commands::hub::hub_snapshot,
            commands::hub::hub_skill_deploy,
            commands::hub::hub_skill_remove,
            commands::hub::hub_skill_import,
            commands::hub::hub_skill_import_many,
            commands::hub::hub_mcp_import,
            commands::hub::hub_mcp_remove,
            commands::hub::hub_mcp_deploy,
            commands::sessions::session_scan,
            commands::sessions::session_page,
            commands::sessions::session_keys,
            commands::sessions::session_facets,
            commands::sessions::session_preview,
            commands::sessions::session_running_clients,
            commands::sessions::session_archive,
            commands::sessions::session_purge,
            commands::sessions::session_retention,
            commands::sessions::session_retention_set,
            commands::activity::activity_overview,
            commands::activity::activity_daily_status,
            commands::activity::activity_daily_hello,
            commands::activity::activity_claim,
            commands::activity::activity_depart,
            commands::activity::activity_pref_set,
            commands::checkin::checkin_credits,
            commands::checkin::checkin_diagnose,
            commands::checkin::credit_trend,
            commands::checkin::credit_claims_recent,
            commands::checkin::credit_flow,
            commands::checkin::checkin_detail,
            commands::checkin::checkin_run,
            commands::checkin::checkin_batch,
            commands::checkin::daily_state,
            commands::checkin::daily_run_now,
            commands::schedule::schedule_list,
            commands::schedule::schedule_save,
            commands::schedule::schedule_delete,
            commands::schedule::schedule_run,
            commands::checkin::zcode_claim_plans,
            commands::checkin::zcode_claim_run,
            commands::logs::log_query,
            commands::logs::log_stats,
            commands::logs::log_export,
            commands::logs::log_clear,
            commands::logs::log_action,
            commands::logs::log_ui,
            commands::settings::log_level,
            commands::settings::config_get,
            commands::settings::config_set,
            commands::system::system_health,
            commands::system::diagnose_bundle,
            commands::system::update_check,
            commands::system::config_snapshot_list,
            commands::system::config_snapshot_restore,
            commands::system::config_snapshot_clear,
            commands::usage::usage_scan,
            commands::usage::usage_overview,
            commands::maintenance::maintenance_space,
            commands::maintenance::maintenance_trash,
            commands::maintenance::maintenance_purge_trash,
            commands::maintenance::maintenance_open_data,
            commands::settings::autostart_get,
            commands::settings::autostart_set
        ])
        .run(tauri::generate_context!())
}

fn init_state(
    instance: Option<process::SingleInstance>,
) -> std::result::Result<AppState, Box<dyn std::error::Error>> {
    log::init(Level::Info)?;
    let db = Arc::new(Db::open()?);

    // 启动体检：版本、账号数、会话索引、数据目录、上一轮轮询结果。
    // 排障先看这一行——原先「数据目录」单独占一行，两行说的是同一件事，合并掉。
    {
        let accounts = crate::app::accounts::repo::list(&db)
            .map(|v| v.len())
            .unwrap_or(0);
        let sessions = crate::app::sessions::store::SqliteSessions::new(&db)
            .count()
            .unwrap_or(0);
        let last_round = db
            .kv_get("daily:last")
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<crate::app::checkin::RoundResult>(&text).ok())
            .map(|r| r.message)
            .unwrap_or_else(|| "从未运行".into());
        log::log(
            Level::Info,
            "app",
            format!(
                "启动体检：v{} · 账号 {accounts} · 会话索引 {sessions} · 数据目录 {} · 上一轮[{last_round}]",
                env!("CARGO_PKG_VERSION"),
                crate::app::paths::data_dir().display()
            ),
        );
    }

    // 跨天清理放在后台：签到记录留 3 天、余额快照留 60 天，不阻塞启动
    let cleanup_db = db.clone();
    tauri::async_runtime::spawn(async move {
        match crate::app::runtime::blocking(move || {
            crate::app::checkin::SqliteCheckin::new(&cleanup_db)
                .cleanup(&crate::app::clock::SystemClock)
        })
        .await
        {
            Ok(Ok(_)) => {}
            // store.rs 的约定是「失败只 warn，不影响主流程」——但得真的 warn
            Ok(Err(e)) => log::log(
                Level::Warn,
                "checkin",
                format!("跨天清理失败：{}", e.message),
            ),
            Err(e) => log::log(Level::Warn, "checkin", format!("跨天清理失败：{e}")),
        }
    });

    // 签到与旅行共用同一个 reqwest client：连接复用，也避免两套超时/证书策略
    let http: Arc<dyn crate::app::net::Http> = Arc::new(crate::app::net::RealHttp::new());
    let checkin = Arc::new(crate::app::checkin::Service::new(
        db.clone(),
        http.clone(),
        Arc::new(crate::app::clock::SystemClock),
    ));
    let activity = Arc::new(crate::app::activity::Service::new(db.clone(), http));

    let sessions = Arc::new(crate::app::sessions::Service::new(
        db.clone(),
        crate::app::clients::Roots::system(),
    ));

    Ok(AppState {
        db,
        checkin,
        activity,
        sessions,
        hub: Arc::new(crate::app::hub::Hub::system()),
        _instance: instance,
    })
}

/// 定时任务调度器：睡到下次执行点，配置变更即时唤醒。
///
/// **定时器必须在 Rust 侧**：前端 `setInterval` 在窗口隐藏/被挂起时就停了，
/// 而后台常驻正是这个工具的主场景。
///
/// 规则本身是用户配的（`app::schedule`，存 kv），这里只负责「到点了就叫起来」，
/// 并且每次触发都落一条 `trigger=schedule` 的执行记录——用户事后能看到
/// 「它自己跑过什么、成没成、花了多久」。
fn spawn_scheduler(
    db: Arc<Db>,
    service: Arc<crate::app::checkin::Service>,
    activity: Arc<crate::app::activity::Service>,
    app: tauri::AppHandle,
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
                let app = app.clone();
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
                        Ok(Ok(Some(item))) => run_schedule(&service, &activity, &app, &item).await,
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
/// 积分刷新每小时一次、旅行轮询每小时一次，全绿时逐条记录只是心跳噪声；
/// 「定时任务到底跑没跑」由「定时任务」页的 last_run 回答，日志只留变化与失败。
pub(crate) struct ScheduleOutcome {
    pub ok: bool,
    pub detail: String,
    pub changed: bool,
}

pub(crate) async fn execute_schedule(
    service: &Arc<crate::app::checkin::Service>,
    activity: &Arc<crate::app::activity::Service>,
    app: &tauri::AppHandle,
    item: &crate::app::schedule::Schedule,
) -> ScheduleOutcome {
    use crate::app::schedule::ScheduleAction::*;
    use tauri::Emitter;

    match item.action {
        CheckinRound => {
            let round = service.run_round("schedule").await;
            // 界面收到就更新底部那一行；收不到也不影响结算
            let _ = app.emit("checkin:round", &round);
            // 签到之后补一次当日活跃：开关默认关，已补过的账号由幂等标记跳过
            let daily = activity.daily_sweep().await;
            let detail = match &daily {
                Some(line) => format!("{}；{line}", round.detail()),
                None => round.detail(),
            };
            ScheduleOutcome {
                ok: round.failed == 0,
                detail,
                changed: round.success > 0 || round.failed > 0 || daily.is_some(),
            }
        }
        CreditsRound => {
            let (total, failed) = service.credits_round().await;
            if total > 0 {
                let _ = app.emit("credits:round", total);
            }
            ScheduleOutcome {
                ok: failed == 0,
                detail: format!("{total} 个账号，失败 {failed}"),
                // 全绿没有可看的信息：余额数字在账号列表上，日志里不必每小时重复一遍
                changed: failed > 0,
            }
        }
        TravelRound => {
            let round = match item.account_iid.as_deref() {
                Some(iid) => activity.round_for("schedule", Some(iid)).await,
                None => activity.round("schedule").await,
            };
            let _ = app.emit("activity:round", &round.message);
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
    app: &tauri::AppHandle,
    item: &crate::app::schedule::Schedule,
) {
    let started = std::time::Instant::now();
    let outcome = execute_schedule(service, activity, app, item).await;
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
fn spawn_zcode_auto_claim(service: Arc<crate::app::checkin::Service>, app: tauri::AppHandle) {
    use tauri::Emitter;

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
                let _ = app.emit("zcode:claim", &line);
            }
        }
    });
}

/// 本地用量采集：启动 15 秒后跑一次，之后每 30 分钟增量扫一遍。
///
/// 只读客户端落盘文件，慢一点没关系；失败只记日志，不影响任何业务。
fn spawn_usage_scan(db: Arc<Db>) {
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

/// 事件桥：日志实时行 → `log:line`；新领取记录 → `claim:recorded`。
fn spawn_event_bridges(app: tauri::AppHandle) {
    use tauri::Emitter;

    let log_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut rx = log::subscribe_live();
        loop {
            match rx.recv().await {
                Ok(line) => {
                    let _ = log_app.emit("log:line", &line);
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
                    let _ = app.emit("claim:recorded", iid);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// 启动自动扫描账号：界面不再提供「扫描客户端」按钮，账号同步改为开机后台执行。
///
/// 延后 2 秒再跑：先让窗口上屏，扫描要读多个客户端目录，不该和冷启动抢 IO。
/// 增量扫描本身很快；失败只记日志，不阻塞窗口。
fn spawn_startup_scan(db: Arc<Db>, app: tauri::AppHandle) {
    use tauri::Emitter;
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
                let _ = app.emit("accounts:scanned", ());
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
fn spawn_hub_auto_sync(db: Arc<Db>, hub: Arc<crate::app::hub::Hub>) {
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

fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let run_checkin = MenuItem::with_id(app, "run-checkin", "立即签到", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "重载界面", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &run_checkin, &reload, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("AI Manager")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            // 不打开窗口也能跑一轮签到：结果照旧进「执行动作」，触发方记 tray
            "run-checkin" => {
                let service = app.state::<AppState>().checkin.clone();
                tauri::async_runtime::spawn(async move {
                    let iids = service.enabled_checkin_iids().await;
                    if iids.is_empty() {
                        log::log(Level::Info, "checkin", "托盘立即签到：没有可签账号");
                        return;
                    }
                    let round = service.batch(&iids, "tray").await;
                    log::log(
                        if round.failed > 0 {
                            Level::Warn
                        } else {
                            Level::Info
                        },
                        "checkin",
                        format!("托盘立即签到：{}", round.detail()),
                    );
                });
            }
            // 界面层挂掉（白屏/卡死）时的自救通道：不动 Rust 侧状态，只重载前端。
            // 先在旧页面盖一层「正在重载」遮罩再刷新——动作发生在眼前，
            // 用户才知道点对了；重载后的确认画面由前端 sessionStorage 标记接力
            "reload" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                    let _ = w.eval(
                        r#"document.getElementById('aim-reload-veil')?.remove();
                        const veil = document.createElement('div');
                        veil.id = 'aim-reload-veil';
                        veil.style.cssText = 'position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;justify-content:center;background:rgba(128,128,128,.25)';
                        veil.innerHTML = '<div style="display:flex;align-items:center;gap:10px;padding:12px 20px;border-radius:10px;background:var(--color-card,#fff);border:1px solid var(--color-line,#ddd);font:500 13px var(--font-sans,system-ui);color:var(--color-fg,#333)"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" style="animation:spin 1s linear infinite"><circle cx="12" cy="12" r="10" stroke="currentColor" stroke-opacity=".25" stroke-width="4"/><path d="M22 12a10 10 0 0 0-10-10" stroke="currentColor" stroke-width="4" stroke-linecap="round"/></svg><span>正在重载界面…</span></div>';
                        const style = document.createElement('style');
                        style.textContent = '@keyframes spin{to{transform:rotate(360deg)}}';
                        document.head.appendChild(style);
                        document.body.appendChild(veil);
                        sessionStorage.setItem('aim.reloading', '1');
                        setTimeout(() => location.reload(), 350);
                        "#,
                    );
                }
            }
            // 退出前把「重复 N 次」的计数补写掉，否则最后一串只剩首行
            "quit" => {
                log::flush();
                app.exit(0)
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 关窗口 = 收到托盘，不退出进程（工作台要常驻）
fn hook_window(app: &mut tauri::App) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = handle.hide();
        }
    });
}

/// 第二个实例启动时，把已有窗口叫到前台。
/// 不做 IPC 握手（要多一套进程间通道），只做「窗口还在就激活」的尽力而为。
fn focus_existing() {
    process::focus_window("AI Manager");
}
