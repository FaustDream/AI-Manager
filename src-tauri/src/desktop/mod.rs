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

pub mod background;
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
            background::spawn_scheduler(sched_db, service.clone(), activity, app.handle().clone());
            background::spawn_startup_scan(scan_db, app.handle().clone());
            background::spawn_hub_auto_sync(sync_db, hub);
            background::spawn_zcode_auto_claim(service, app.handle().clone());
            background::spawn_event_bridges(app.handle().clone());
            background::spawn_usage_scan(usage_db);

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
            commands::settings::network_selfcheck,
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
