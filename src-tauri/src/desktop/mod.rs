//! HTTP 壳层：状态装配 + 命令面 + 后台任务。
//!
//! 浏览器化迁移（2026-11-18）后桌面壳已退役：没有窗口、托盘与自启，
//! 界面全部由浏览器承载（`web.rs` 托管前端产物 + `/cmd` + `/events`）。
//! 本层**只做转接**，不放业务逻辑——所有能力来自 `app/` 下的模块。
//! 命令的「类型」由 ts-rs 生成，「名单」由 `web.rs` 的分发表保证（架构 D8）。
//!
//! 两条 P1 就得立住的规矩：
//!   - 单实例锁必须活到进程结束：双开会并发写同一个 app.db、把签到跑成双份
//!   - 数据库调用一律 `spawn_blocking`，否则命令通道会在等结果期间卡住

use crate::app::db::Db;
use crate::app::error::AppError;
use crate::app::log::{self, Level};
use crate::app::process;
use std::sync::Arc;

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
pub mod web;

/// 状态装配：日志、数据库、三个服务与 Hub。
/// `pub` 是给 bin 入口与集成测试用的——装配必须与生产同源。
pub fn init_state(
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
                "启动体检：账号 {accounts} · 会话索引 {sessions} · 数据目录 {} · 上一轮[{last_round}]",
                crate::app::paths::data_dir().display()
            ),
        );
    }

    // 跨天清理放在后台：签到记录留 3 天、余额快照留 60 天，不阻塞启动
    let cleanup_db = db.clone();
    crate::app::runtime::runtime().spawn(async move {
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
