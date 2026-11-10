//! 网页调试模式入口：与桌面壳共用同一套 AppState、命令面与后台任务，
//! 把命令挂到本机 HTTP（`desktop::web`），前端在浏览器里直连。
//!
//! 与桌面壳**共用同一个单实例互斥体**：两个壳的调度器同时跑会对同一个
//! app.db 并发写、把签到跑成双份，所以这里宁可拒绝启动。
//!
//! 运行：`cargo run --bin ai-manager-web`（debug 构建的数据目录仍指向仓库根 data/）。

use ai_manager::app::config;
use ai_manager::app::log;
use ai_manager::app::process;
use ai_manager::app::runtime;
use ai_manager::desktop::{self, background, web};
use std::sync::Arc;

fn main() {
    // 实例名可由环境变量覆盖：正式版（桌面壳）持的是同名互斥体，开发期需要两者并存，
    // 用独立名字各持一把锁即可互不干扰。缺省仍取正式名，正式行为不变。
    let instance_name = std::env::var("AIMANAGER_INSTANCE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "AIManager.SingleInstance".to_string());
    let instance = match process::acquire_single_instance(&instance_name) {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => {
            eprintln!("已有 AI Manager 实例在运行（桌面版与网页模式共用同一个实例锁）。");
            eprintln!("请先退出正在运行的实例，再启动网页调试模式。");
            std::process::exit(1);
        }
        Err(e) => {
            // 与桌面壳同口径：拿不到锁不硬退，但必须留痕
            eprintln!("单实例互斥体获取失败（{}），双开保护失效", e.message);
            None
        }
    };

    let state = match desktop::init_state(instance) {
        Ok(state) => Arc::new(state),
        Err(e) => {
            eprintln!("初始化失败：{e}");
            std::process::exit(1);
        }
    };
    // 应用配置：加载 data/config.toml（与桌面壳 setup 同一步）
    config::init(&state.db);

    // 后台任务与桌面壳 setup 完全同构；事件走统一总线（源侧桥），SSE 自己订阅
    let (db, checkin, activity, hub) = (
        state.db.clone(),
        state.checkin.clone(),
        state.activity.clone(),
        state.hub.clone(),
    );
    background::spawn_scheduler(db.clone(), checkin.clone(), activity);
    background::spawn_startup_scan(db.clone());
    background::spawn_hub_auto_sync(db.clone(), hub);
    background::spawn_zcode_auto_claim(checkin);
    background::spawn_source_bridges();
    background::spawn_usage_scan(db);
    background::spawn_session_scan(state.sessions.clone());

    log::action("app.startup", "startup", true, 0, "网页调试模式已启动");
    println!("AI Manager 网页调试模式：");
    println!("  1. 另开终端执行 npm run dev（前端 dev server：http://127.0.0.1:5273）");
    println!("  2. 浏览器打开 http://127.0.0.1:5273 即可测试");
    println!("     命令通道 /cmd/<命令名> · 事件通道 /events（SSE）· Ctrl+C 退出");

    // 状态整体移交给壳（manage 进运行时）；后台任务只持有 Arc 字段克隆，
    // 到这里外层 Arc 计数应已归 1
    let state = match Arc::try_unwrap(state) {
        Ok(state) => state,
        Err(_) => {
            eprintln!("AppState 仍被后台任务持有，无法移交给网页壳");
            std::process::exit(1);
        }
    };
    let bind = match runtime::runtime()
        .block_on(tokio::net::TcpListener::bind(("127.0.0.1", web::WEB_PORT)))
    {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!(
                "端口 {} 被占用（多半是上一个网页调试壳还没退）。先结束它再启动。",
                web::WEB_PORT
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("绑定 127.0.0.1:{} 失败：{e}", web::WEB_PORT);
            std::process::exit(1);
        }
    };
    // 真实 Wry 运行时：必须在主线程 build（tao 要求），且从不 run()——
    // 只借它的 Manager 语义把 AppState 变成命令函数认识的 State
    let app = match tauri::Builder::<tauri::Wry>::new().build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("网页壳运行时初始化失败：{e}");
            std::process::exit(1);
        }
    };
    match runtime::runtime().block_on(web::serve(bind, app, state)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("HTTP 服务退出：{e}");
            std::process::exit(1);
        }
    }
}
