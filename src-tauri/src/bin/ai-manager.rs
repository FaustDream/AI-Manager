//! AI Manager 入口：本机后端 + 浏览器界面。
//!
//! 进程持有单实例锁（防双开调度器写同一 app.db），装配 AppState 后
//! 拉起全部后台任务，把命令面与前端静态产物挂到本机 HTTP（`desktop::web`）。
//! 界面在浏览器里：start.cmd 负责拉起本进程并打开页面。
//!
//! 运行：`cargo run`（debug 构建的数据目录指向仓库根 data/，前端产物指仓库根 dist/）。

use ai_manager::app::config;
use ai_manager::app::log;
use ai_manager::app::process;
use ai_manager::app::runtime;
use ai_manager::desktop::{self, background, web};
use std::sync::Arc;

fn main() {
    // 实例名可由环境变量覆盖：需要并存两个实例时各持一把锁即可互不干扰。
    let instance_name = std::env::var("AIMANAGER_INSTANCE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "AIManager.SingleInstance".to_string());
    let instance = match process::acquire_single_instance(&instance_name) {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => {
            eprintln!("已有 AI Manager 实例在运行（单实例锁防双开调度器）。");
            eprintln!("请先退出正在运行的实例，再启动。");
            std::process::exit(1);
        }
        Err(e) => {
            // 拿不到锁不硬退，但必须留痕
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
    // 应用配置：加载 data/config.toml
    config::init(&state.db);

    // 后台任务：调度器、启动扫描、技能同步、ZCode 领取、事件源桥、
    // 用量采集、会话扫描、模型接口自启、资料库最新更新热刷新。
    // 事件经统一总线 → SSE 送浏览器。
    let (db, checkin, activity, hub, catalog) = (
        state.db.clone(),
        state.checkin.clone(),
        state.activity.clone(),
        state.hub.clone(),
        state.catalog.clone(),
    );
    background::spawn_scheduler(db.clone(), checkin.clone(), activity, state.catalog.clone());
    background::spawn_startup_scan(db.clone());
    background::spawn_hub_auto_sync(db.clone(), hub);
    background::spawn_zcode_auto_claim(checkin);
    background::spawn_source_bridges();
    background::spawn_usage_scan(db.clone());
    background::spawn_session_scan(state.sessions.clone());
    background::spawn_model_api(db);
    background::spawn_catalog_hot_sync(catalog.clone());
    background::spawn_catalog_dedupe_backfill(catalog);

    log::action(
        "app.startup",
        "startup",
        true,
        0,
        "后端已启动（浏览器界面）",
    );
    println!("AI Manager 后端已启动：");
    println!("  浏览器打开 http://127.0.0.1:{} 即可使用", web::WEB_PORT);
    println!("  页面 + 命令通道 /cmd/<命令名> · 事件通道 /events（SSE）· Ctrl+C 退出");

    // 状态整体移交给壳；后台任务只持有 Arc 字段克隆，
    // 到这里外层 Arc 计数应已归 1
    let state = match Arc::try_unwrap(state) {
        Ok(state) => state,
        Err(_) => {
            eprintln!("AppState 仍被后台任务持有，无法移交给 HTTP 壳");
            std::process::exit(1);
        }
    };
    let bind = match runtime::runtime()
        .block_on(tokio::net::TcpListener::bind(("127.0.0.1", web::WEB_PORT)))
    {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!(
                "端口 {} 被占用（多半是上一个实例还没退）。先结束它再启动。",
                web::WEB_PORT
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("绑定 127.0.0.1:{} 失败：{e}", web::WEB_PORT);
            std::process::exit(1);
        }
    };
    match runtime::runtime().block_on(web::serve(bind, state)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("HTTP 服务退出：{e}");
            std::process::exit(1);
        }
    }
}
