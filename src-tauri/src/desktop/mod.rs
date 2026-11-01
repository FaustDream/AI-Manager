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
use crate::app::task::{TaskState, TaskStore, TaskView};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State, WindowEvent};

pub struct AppState {
    pub db: Arc<Db>,
    pub tasks: Arc<TaskStore>,
    /// 签到编排。持有网络与时钟，命令层只转接
    pub checkin: Arc<crate::app::checkin::Service>,
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

fn err_text(e: AppError) -> String {
    // 前端拿到的永远是 {kind, message} 的结构化错误
    serde_json::to_string(&e).unwrap_or(e.message)
}

// ------------------------------------------------------------------ 命令

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 前端挂载完成后回调一次。
/// 意义不在于「打个招呼」，而在于让 Rust 侧能确认 IPC 通道真的通了——
/// 窗口能开但 `__TAURI__` 没注进来的情况，只看窗口是看不出来的。
#[tauri::command]
fn ui_ready(state: State<'_, AppState>) -> String {
    log::log(Level::Info, "ui", "前端已就绪，IPC 通道正常");
    format!("{}|{}", env!("CARGO_PKG_VERSION"), state.db_path_label())
}

#[tauri::command]
fn app_quit(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn window_show(app: tauri::AppHandle) -> std::result::Result<(), String> {
    let Some(w) = app.get_webview_window("main") else {
        return Err(err_text(AppError::new("invalid", "主窗口不存在")));
    };
    w.show()
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    w.set_focus()
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    Ok(())
}

#[tauri::command]
fn window_hide(app: tauri::AppHandle) -> std::result::Result<(), String> {
    let Some(w) = app.get_webview_window("main") else {
        return Err(err_text(AppError::new("invalid", "主窗口不存在")));
    };
    w.hide()
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    Ok(())
}

#[tauri::command]
fn tray_set_autostart(enabled: bool) -> std::result::Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    process::set_autostart(enabled, &exe).map_err(err_text)
}

#[tauri::command]
async fn settings_get(
    state: State<'_, AppState>,
    key: String,
) -> std::result::Result<Option<serde_json::Value>, String> {
    let db = state.db.clone();
    let raw = crate::app::runtime::blocking(move || db.kv_get(&key))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)?;
    Ok(raw.and_then(|text| serde_json::from_str(&text).ok()))
}

#[tauri::command]
async fn settings_set(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> std::result::Result<(), String> {
    let db = state.db.clone();
    let text = value.to_string();
    crate::app::runtime::blocking(move || db.kv_set(&key, &text))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

#[tauri::command]
fn task_list(state: State<'_, AppState>) -> Vec<TaskView> {
    state.tasks.list()
}

// ------------------------------------------------------------------ 账号中心

#[tauri::command]
async fn account_list(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::accounts::AccountView>, String> {
    let db = state.db.clone();
    // 数据库调用必须在阻塞线程里跑，否则界面在等结果期间没有响应
    let mut rows = crate::app::runtime::blocking(move || {
        let mut rows = crate::app::accounts::repo::list(&db)?;
        // 余额 / 今日已签 从库里补出来：重扫与重启都不会把它们冲掉（契约 21）
        crate::app::checkin::store::hydrate(&db, &mut rows, &crate::app::clock::SystemClock);
        Ok::<_, AppError>(rows)
    })
    .await
    .map_err(|e| err_text(AppError::new("io", e.to_string())))?
    .map_err(err_text)?;
    // 脱敏在出口做一次：前端不必各自实现，也就不会有人漏掉
    for row in &mut rows {
        row.name = row.display_name();
    }
    log::log(
        Level::Info,
        "accounts",
        format!("列表返回 {} 个账号", rows.len()),
    );
    Ok(rows)
}

#[tauri::command]
async fn account_reorder(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<(), String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::accounts::repo::reorder(&db, &iids))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

#[tauri::command]
async fn account_delete(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<bool, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::accounts::repo::delete(&db, &iid))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

/// 扫描各客户端登录态并入库。
/// 走 `spawn_blocking`：读客户端库 + DPAPI/AES 解密是纯阻塞活，放主线程会让窗口变灰。
#[tauri::command]
async fn account_scan(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::accounts::ScanReport, String> {
    let db = state.db.clone();
    let report = crate::app::runtime::blocking(move || crate::app::accounts::scan_all(&db))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)?;
    log::log(
        Level::Info,
        "accounts",
        format!(
            "扫描完成：{} 个账号（新增 {}）",
            report.scanned, report.added
        ),
    );
    Ok(report)
}

/// 切换登录。会写客户端登录态文件：失败一律带结构化 kind，界面按 kind 给话术。
#[tauri::command]
async fn account_switch(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::accounts::SwitchResult, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::accounts::switch(&db, &iid))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

/// 各客户端当前登录的账号（界面给行标「当前」）。
#[tauri::command]
async fn account_current() -> std::result::Result<Vec<String>, String> {
    let current = crate::app::runtime::blocking(crate::app::accounts::current_iids)
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    log::log(
        Level::Info,
        "accounts",
        format!("客户端当前登录：{} 个", current.len()),
    );
    Ok(current)
}

/// 添加账号第一步：退出该平台登录态并启动客户端。
/// 第二步是用户自己在客户端里登录，然后点「扫描客户端」——见 `accounts::add` 的说明。
#[tauri::command]
async fn account_add_prepare(platform: String) -> std::result::Result<String, String> {
    crate::app::runtime::blocking(move || crate::app::accounts::add::prepare(&platform))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

#[tauri::command]
fn task_cancel(state: State<'_, AppState>, task_id: u64) -> bool {
    state.tasks.cancel_task(task_id)
}

// ------------------------------------------------------------ 技能与 MCP

fn target_keys(names: &[String]) -> Vec<crate::contracts::TargetKey> {
    names
        .iter()
        .filter_map(|name| crate::app::hub::targets::from_key_name(name))
        .collect()
}

/// 技能 + MCP + 平台清单，一次读完。
#[tauri::command]
async fn hub_snapshot(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::hub::HubSnapshot, String> {
    let hub = state.hub.clone();
    crate::app::runtime::blocking(move || crate::app::hub::snapshot(&hub))
        .await
        .map_err(err_text)
}

/// 部署技能到选中的平台。`overwrite=true` 才允许盖掉内容不同的同名副本。
#[tauri::command]
async fn hub_skill_deploy(
    state: State<'_, AppState>,
    name: String,
    targets: Vec<String>,
    overwrite: Option<bool>,
) -> std::result::Result<crate::app::hub::DeployReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    let overwrite = overwrite.unwrap_or(false);
    let report = crate::app::runtime::blocking(move || {
        crate::app::hub::skills::deploy(&hub, &name, &keys, overwrite)
    })
    .await
    .map_err(err_text)?
    .map_err(err_text)?;
    log::log(
        Level::Info,
        "hub",
        format!("部署技能：{}", report.summary()),
    );
    Ok(report)
}

/// 从平台移除技能（只删本工具部署的副本）。
#[tauri::command]
async fn hub_skill_remove(
    state: State<'_, AppState>,
    name: String,
    targets: Vec<String>,
) -> std::result::Result<crate::app::hub::DeployReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    let report =
        crate::app::runtime::blocking(move || crate::app::hub::skills::remove(&hub, &name, &keys))
            .await
            .map_err(err_text)?;
    log::log(
        Level::Info,
        "hub",
        format!("移除技能：{}", report.summary()),
    );
    Ok(report)
}

/// 从某个平台把技能导入库。
#[tauri::command]
async fn hub_skill_import(
    state: State<'_, AppState>,
    target: String,
    name: String,
) -> std::result::Result<crate::app::hub::skills::SkillInfo, String> {
    let hub = state.hub.clone();
    let key = crate::app::hub::targets::from_key_name(&target)
        .ok_or_else(|| err_text(AppError::new("invalid", format!("未知平台：{target}"))))?;
    crate::app::runtime::blocking(move || crate::app::hub::skills::import(&hub, key, &name))
        .await
        .map_err(err_text)?
        .map_err(err_text)
}

#[derive(serde::Serialize)]
pub struct McpImportReport {
    pub names: Vec<String>,
    pub report: crate::app::hub::DeployReport,
}

/// 粘贴一段 MCP 配置，写到选中的平台。
#[tauri::command]
async fn hub_mcp_import(
    state: State<'_, AppState>,
    text: String,
    targets: Vec<String>,
) -> std::result::Result<McpImportReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    if keys.is_empty() {
        return Err(err_text(AppError::new("invalid", "先选至少一个平台")));
    }
    crate::app::runtime::blocking(move || {
        let entries = crate::app::hub::parse_text(&text)?;
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        let mut report = crate::app::hub::DeployReport::default();
        for entry in &entries {
            let one = crate::app::hub::mcp_add(&hub, entry, &keys);
            report.steps.extend(one.steps);
        }
        Ok(McpImportReport { names, report })
    })
    .await
    .map_err(err_text)?
    .map_err(err_text)
}

/// 从选中的平台删掉一条 MCP（只删本工具写的）。
#[tauri::command]
async fn hub_mcp_remove(
    state: State<'_, AppState>,
    name: String,
    targets: Vec<String>,
) -> std::result::Result<crate::app::hub::DeployReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    crate::app::runtime::blocking(move || crate::app::hub::mcp_remove(&hub, &name, &keys))
        .await
        .map_err(err_text)
}

/// 把一条已存在的 MCP 条目写到更多平台（MCP 表每行的「部署」）。
#[tauri::command]
async fn hub_mcp_deploy(
    state: State<'_, AppState>,
    name: String,
    targets: Vec<String>,
) -> std::result::Result<crate::app::hub::DeployReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    crate::app::runtime::blocking(move || crate::app::hub::mcp_deploy(&hub, &name, &keys))
        .await
        .map_err(err_text)
}

// ------------------------------------------------------------ 会话

/// 增量扫描会话索引：任务化，扫完 emit `session:scan`。
#[tauri::command]
async fn session_scan(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> std::result::Result<u64, String> {
    use crate::app::task::CapabilityId;
    use tauri::Emitter;

    let service = state.sessions.clone();
    let emitter = app.clone();
    let task_id = state.tasks.spawn_managed(
        CapabilityId::Sessions,
        "扫描会话索引",
        move |handle| async move {
            match service.scan(handle.cancel.clone()).await {
                Ok(report) => {
                    log::log(
                        Level::Info,
                        "sessions",
                        format!("会话扫描：{}", report.summary()),
                    );
                    handle.message(report.summary());
                    let _ = emitter.emit("session:scan", &report);
                    Ok(())
                }
                Err(e) => {
                    log::log(
                        Level::Warn,
                        "sessions",
                        format!("会话扫描失败：{}", e.message),
                    );
                    let _ = emitter.emit("session:scan", &e);
                    Err(e)
                }
            }
        },
    );
    Ok(task_id)
}

/// 列表页：只读 app.db，客户端正在写也不影响。
#[tauri::command]
async fn session_page(
    state: State<'_, AppState>,
    cursor: Option<String>,
    limit: Option<usize>,
    filter: Option<crate::app::sessions::Filter>,
) -> std::result::Result<crate::types::Page<crate::types::SessionRow>, String> {
    let service = state.sessions.clone();
    service
        .page(filter.unwrap_or_default(), cursor, limit.unwrap_or(100))
        .await
        .map_err(err_text)
}

/// 正文预览：按行 key 找正文。只读、只取头尾。
#[tauri::command]
async fn session_preview(
    state: State<'_, AppState>,
    key: String,
) -> std::result::Result<crate::app::sessions::Preview, String> {
    let service = state.sessions.clone();
    service.preview(key).await.map_err(err_text)
}

/// 删除会话：进回收站，可恢复。会临时关掉正在运行的客户端。
#[tauri::command]
async fn session_delete(
    state: State<'_, AppState>,
    keys: Vec<String>,
) -> std::result::Result<crate::app::sessions::delete::DeleteReport, String> {
    let service = state.sessions.clone();
    let report = service.remove(keys).await.map_err(err_text)?;
    log::log(
        if report.errors.is_empty() {
            Level::Info
        } else {
            Level::Warn
        },
        "sessions",
        format!("删除会话：{}", report.summary()),
    );
    Ok(report)
}

/// 整库快照（备份与清理页也会用）。
#[tauri::command]
async fn session_backup(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::sessions::backup::Snapshot, String> {
    let service = state.sessions.clone();
    let snapshot = service.backup().await.map_err(err_text)?;
    log::log(
        Level::Info,
        "sessions",
        format!("会话快照：{}", snapshot.summary()),
    );
    Ok(snapshot)
}

#[tauri::command]
async fn session_backups(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::sessions::backup::Snapshot>, String> {
    let service = state.sessions.clone();
    Ok(service.backups().await)
}

/// 回收站批次列表。
#[tauri::command]
async fn session_trash(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::sessions::trash::Manifest>, String> {
    let service = state.sessions.clone();
    Ok(service.trash().await)
}

/// 按批次恢复。
#[tauri::command]
async fn session_trash_restore(
    state: State<'_, AppState>,
    tag: String,
) -> std::result::Result<crate::app::sessions::delete::RestoreReport, String> {
    let service = state.sessions.clone();
    service.trash_restore(tag).await.map_err(err_text)
}

/// 彻底清掉一个批次，不可恢复。
#[tauri::command]
async fn session_trash_purge(
    state: State<'_, AppState>,
    tag: String,
) -> std::result::Result<i64, String> {
    let service = state.sessions.clone();
    service.trash_purge(tag).await.map_err(err_text)
}

/// 三个下拉的取值与计数（来源 / 账号 / 状态）。
#[tauri::command]
async fn session_facets(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::sessions::Facets, String> {
    let service = state.sessions.clone();
    service.facets().await.map_err(err_text)
}

// ------------------------------------------------------------ 签到与积分

/// 只查积分，不动签到状态（「刷新积分」走这条）。
#[tauri::command]
async fn checkin_credits(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<Vec<(String, crate::types::CreditSnapshot)>, String> {
    // 并发与失败兜底都在 Service::credits_batch 里，命令层只转接
    Ok(state.checkin.credits_batch(&iids).await)
}

/// 详情面板：余额、今日增减、批次、下次可签。只读本地库，不打接口。
#[tauri::command]
async fn checkin_detail(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::checkin::AccountDetail, String> {
    let service = state.checkin.clone();
    Ok(service.detail(&iid).await)
}

/// 单账号签到。**会真的调接口**，幂等由 daily_marks 保证。
#[tauri::command]
async fn checkin_run(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::types::ActionResult, String> {
    let service = state.checkin.clone();
    let result = service.checkin(&iid).await.map_err(err_text)?;
    log::log(
        if result.ok { Level::Info } else { Level::Warn },
        "checkin",
        format!(
            "{}：{}",
            iid.split(':').next().unwrap_or("-"),
            result.message
        ),
    );
    Ok(result)
}

/// 批量签到：任务化，可取消，并发 3。
#[tauri::command]
async fn checkin_batch(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<u64, String> {
    use crate::app::task::CapabilityId;

    let total = iids.len();
    let service = state.checkin.clone();
    let task_id = state.tasks.spawn_managed(
        CapabilityId::AccountCenter,
        format!("批量签到 {total} 个账号"),
        move |handle| async move {
            let cancel = handle.cancel.clone();
            let round = service
                .batch(&iids, cancel, |done, total| {
                    handle.progress(
                        done as f32 / total.max(1) as f32,
                        format!("{done}/{total}"),
                    );
                })
                .await;
            let message = round.message.clone();
            handle.message(message.clone());
            log::log(Level::Info, "checkin", format!("批量签到结束：{message}"));
            // 结束态交给统一归类：全失败记 Failed、部分失败记 Partial、取消记 Cancelled
            if handle.cancel.is_cancelled() {
                Err(crate::app::error::AppError::new("cancelled", message))
            } else if round.failed > 0 && round.success == 0 {
                Err(crate::app::error::AppError::new("io", message))
            } else if round.failed > 0 {
                Err(crate::app::error::AppError::new("conflict", message))
            } else {
                Ok(())
            }
        },
    );
    Ok(task_id)
}

/// 每日轮询的最近一轮结果（界面底部那一行）。
#[tauri::command]
fn daily_state(state: State<'_, AppState>) -> Option<crate::app::checkin::RoundResult> {
    state.checkin.daily_state()
}

/// 手动跑一轮（会在后台执行，返回任务 id）。
#[tauri::command]
async fn daily_run_now(state: State<'_, AppState>) -> std::result::Result<u64, String> {
    use crate::app::task::CapabilityId;

    let service = state.checkin.clone();
    let task_id = state.tasks.spawn_managed(
        CapabilityId::AccountCenter,
        "每日轮询（手动）",
        move |handle| async move {
            let round = service.run_round(&handle.cancel).await;
            log::log(
                if round.failed > 0 {
                    Level::Warn
                } else {
                    Level::Info
                },
                "checkin",
                format!("每日轮询：{}", round.message),
            );
            handle.message(round.message.clone());
            if handle.cancel.is_cancelled() {
                Err(crate::app::error::AppError::new("cancelled", round.message))
            } else {
                Ok(())
            }
        },
    );
    Ok(task_id)
}

/// ZCode 可领取的活动套餐（v1 的「领取」按钮第一步）。
#[tauri::command]
async fn zcode_claim_plans(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<Vec<crate::app::providers::zclaim::ClaimPlan>, String> {
    let service = state.checkin.clone();
    service.zcode_claimable(&iid).await.map_err(err_text)
}

/// 领取一个 ZCode 活动套餐：**会打开浏览器过验证码**，验证完自动提交。
#[tauri::command]
async fn zcode_claim_run(
    state: State<'_, AppState>,
    iid: String,
    plan_id: String,
) -> std::result::Result<crate::app::providers::zclaim::ClaimOutcome, String> {
    let service = state.checkin.clone();
    service.zcode_claim(&iid, &plan_id).await.map_err(err_text)
}

/// 结构化日志查询（按级别 / 模块 / 关键词）。
/// 返回 `Result`：读日志失败要让界面说「读日志失败」，不能伪装成「没有符合条件的日志」。
#[tauri::command]
async fn log_query(
    filter: crate::app::log::LogFilter,
) -> std::result::Result<Vec<crate::app::log::Line>, String> {
    // query 是整读 app.jsonl 再逐行过滤，必须进阻塞池
    crate::app::runtime::blocking(move || crate::app::log::query(&filter))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

/// 导出日志到文件；不给路径就落到 `data/logs/export-<时间戳>.txt`。
/// 返回一句可以直接显示的话（行数 + 落在哪）。
#[tauri::command]
async fn log_export(
    path: Option<String>,
    filter: crate::app::log::LogFilter,
) -> std::result::Result<String, String> {
    let target = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => std::path::PathBuf::from(p),
        None => crate::app::paths::logs_dir().join(format!(
            "export-{}.txt",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        )),
    };
    let shown = target.display().to_string();
    let count = crate::app::runtime::blocking(move || crate::app::log::export(&target, &filter))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)?;
    Ok(format!("已导出 {count} 行到 {shown}"))
}

/// 清掉已完成 / 失败 / 取消的任务，返回清掉几条。
#[tauri::command]
fn task_clear_finished(state: State<'_, AppState>) -> usize {
    state.tasks.clear_finished()
}

/// 数据目录占用。
#[tauri::command]
async fn maintenance_space() -> crate::app::maintenance::SpaceReport {
    // 递归遍历整个 data/（含会话正文），主线程上跑会卡住界面
    crate::app::runtime::blocking(crate::app::maintenance::space)
        .await
        .unwrap_or_default()
}

/// 回收站批次（会话删除与平台技能移除都算）。
#[tauri::command]
async fn maintenance_trash() -> Vec<crate::app::maintenance::SpaceEntry> {
    crate::app::runtime::blocking(crate::app::maintenance::trash_batches)
        .await
        .unwrap_or_default()
}

/// 清空回收站：`tag=None` 全清。返回释放的字节数。
#[tauri::command]
async fn maintenance_purge_trash(tag: Option<String>) -> std::result::Result<i64, String> {
    crate::app::runtime::blocking(move || crate::app::maintenance::purge_trash(tag.as_deref()))
        .await
        .map_err(|e| err_text(AppError::new("io", e.to_string())))?
        .map_err(err_text)
}

/// 打开数据目录。
#[tauri::command]
fn maintenance_open_data() -> std::result::Result<(), String> {
    crate::app::maintenance::open_data_dir().map_err(err_text)
}

/// 迁移收尾第一步：三项计数校验。只读，随时可点。
#[tauri::command]
async fn migrate_verify(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::migrate::verify::MigrationReport, String> {
    let db = state.db.clone();
    // 读 accounts.dat 要过 DPAPI、还要解 JSON，纯阻塞活，不能占着异步线程
    let report = crate::app::runtime::blocking(move || {
        crate::app::migrate::verify::verify(&db, &crate::app::paths::data_dir())
    })
    .await
    .map_err(|e| err_text(AppError::new("io", e.to_string())))?
    .map_err(err_text)?;
    log::log(
        if report.ok { Level::Info } else { Level::Warn },
        "migrate",
        format!("迁移校验：{}", report.describe()),
    );
    Ok(report)
}

/// 迁移收尾第二步：**复制** v1 文件到 `data-v1-archive/<时间戳>/`。
/// 计数不一致直接拒绝，什么都不写；原文件永不移动、永不删除。
#[tauri::command]
async fn migrate_archive(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::migrate::verify::ArchiveReport, String> {
    let db = state.db.clone();
    let report = crate::app::runtime::blocking(move || {
        crate::app::migrate::verify::archive(&db, &crate::app::paths::data_dir())
    })
    .await
    .map_err(|e| err_text(AppError::new("io", e.to_string())))?
    .map_err(err_text)?;
    log::log(
        Level::Info,
        "migrate",
        format!("v1 文件已归档到 {}", report.archive_dir),
    );
    Ok(report)
}

#[tauri::command]
fn autostart_get() -> bool {
    crate::app::process::autostart_enabled()
}

#[tauri::command]
fn autostart_set(enabled: bool) -> std::result::Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| err_text(AppError::new("io", e.to_string())))?;
    crate::app::process::set_autostart(enabled, &exe).map_err(err_text)
}

#[tauri::command]
fn log_level(level: String) -> std::result::Result<(), String> {
    let parsed = Level::parse_lossy(&level);
    log::set_level(parsed);
    Ok(())
}

// ------------------------------------------------------------------ 启动

pub fn run() -> tauri::Result<()> {
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
            let hub = state.hub.clone();
            let sync_db = state.db.clone();
            let claim_db = state.db.clone();
            let state_tasks = state.tasks.clone();
            app.manage(state);

            build_tray(app)?;
            hook_window(app);
            spawn_daily_round(service.clone(), state_tasks.clone(), app.handle().clone());
            spawn_credits_round(service.clone(), state_tasks.clone(), app.handle().clone());
            spawn_hub_auto_sync(sync_db, hub);
            spawn_zcode_auto_claim(claim_db, service, app.handle().clone());
            spawn_event_bridges(app.handle().clone(), state_tasks);

            log::log(Level::Info, "app", "P1 外壳启动完成");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            ui_ready,
            app_quit,
            window_show,
            window_hide,
            tray_set_autostart,
            settings_get,
            settings_set,
            task_list,
            task_cancel,
            account_list,
            account_reorder,
            account_delete,
            account_scan,
            account_switch,
            account_current,
            account_add_prepare,
            hub_snapshot,
            hub_skill_deploy,
            hub_skill_remove,
            hub_skill_import,
            hub_mcp_import,
            hub_mcp_remove,
            hub_mcp_deploy,
            session_scan,
            session_page,
            session_facets,
            session_preview,
            session_delete,
            session_backup,
            session_backups,
            session_trash,
            session_trash_restore,
            session_trash_purge,
            checkin_credits,
            checkin_detail,
            checkin_run,
            checkin_batch,
            daily_state,
            daily_run_now,
            zcode_claim_plans,
            zcode_claim_run,
            log_query,
            log_export,
            log_level,
            task_clear_finished,
            maintenance_space,
            maintenance_trash,
            maintenance_purge_trash,
            maintenance_open_data,
            migrate_verify,
            migrate_archive,
            autostart_get,
            autostart_set
        ])
        .run(tauri::generate_context!())
}

fn init_state(
    instance: Option<process::SingleInstance>,
) -> std::result::Result<AppState, Box<dyn std::error::Error>> {
    log::init(Level::Info)?;
    let db = Arc::new(Db::open()?);
    let tasks = TaskStore::new();

    log::log(
        Level::Info,
        "app",
        format!("数据目录 {}", crate::app::paths::data_dir().display()),
    );

    // 启动体检：账号数、会话索引、上一轮轮询结果，排查问题先看这一行
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
            format!("启动体检：账号 {accounts} · 会话索引 {sessions} · 上一轮[{last_round}]"),
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
            Ok(Err(e)) => log::log(Level::Warn, "checkin", format!("跨天清理失败：{}", e.message)),
            Err(e) => log::log(Level::Warn, "checkin", format!("跨天清理失败：{e}")),
        }
    });

    let checkin = Arc::new(crate::app::checkin::Service::new(
        db.clone(),
        Arc::new(crate::app::net::RealHttp::new()),
        Arc::new(crate::app::clock::SystemClock),
    ));

    let sessions = Arc::new(crate::app::sessions::Service::new(
        db.clone(),
        crate::app::clients::Roots::system(),
    ));

    Ok(AppState {
        db,
        tasks,
        checkin,
        sessions,
        hub: Arc::new(crate::app::hub::Hub::system()),
        _instance: instance,
    })
}

/// 每日轮询：启动后跑一轮，之后每 6 小时一轮。
///
/// **定时器必须在 Rust 侧**：前端 `setInterval` 在窗口隐藏/被挂起时就停了，
/// 而后台常驻正是这个工具的主场景。
fn spawn_daily_round(
    service: Arc<crate::app::checkin::Service>,
    tasks: Arc<crate::app::task::TaskStore>,
    app: tauri::AppHandle,
) {
    use crate::app::task::CapabilityId;
    use tauri::Emitter;

    tauri::async_runtime::spawn(async move {
        // 等首屏稳定再跑，别和账号列表抢 SQLite 的读连接
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        loop {
            // 定时轮询也走任务表：设置页的任务列表能看到它跑过、跑了什么
            tasks.spawn_managed(CapabilityId::AccountCenter, "每日轮询（定时）", {
                let service = service.clone();
                let app = app.clone();
                move |handle| async move {
                    let round = service.run_round(&handle.cancel).await;
                    log::log(
                        if round.failed > 0 {
                            Level::Warn
                        } else {
                            Level::Info
                        },
                        "checkin",
                        format!("每日轮询：{}", round.message),
                    );
                    // 界面收到就更新底部那一行；收不到也不影响结算
                    let _ = app.emit("checkin:round", &round);
                    handle.message(round.message.clone());
                    if handle.cancel.is_cancelled() {
                        Err(crate::app::error::AppError::new("cancelled", round.message))
                    } else {
                        Ok(())
                    }
                }
            });
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });
}

/// 积分定时刷新：每小时一轮。签到轮询一天只有 4 次带余额，
/// 界面上的余额要更跟手；失败逐条进错误快照，不打断整轮。
fn spawn_credits_round(
    service: Arc<crate::app::checkin::Service>,
    tasks: Arc<crate::app::task::TaskStore>,
    app: tauri::AppHandle,
) {
    use crate::app::task::CapabilityId;
    use tauri::Emitter;

    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            tasks.spawn_managed(CapabilityId::AccountCenter, "积分定时刷新", {
                let service = service.clone();
                let app = app.clone();
                move |handle| async move {
                    let (total, failed) = service.credits_round().await;
                    if total > 0 {
                        log::log(
                            if failed > 0 { Level::Warn } else { Level::Info },
                            "checkin",
                            format!("积分刷新：{total} 个账号，失败 {failed}"),
                        );
                        let _ = app.emit("credits:round", total);
                    }
                    handle.message(format!("{total} 个账号，失败 {failed}"));
                    if failed > 0 && failed == total {
                        Err(crate::app::error::AppError::new(
                            "io",
                            format!("全部 {total} 个账号都刷新失败"),
                        ))
                    } else {
                        Ok(())
                    }
                }
            });
        }
    });
}

/// ZCode 活动自动领取（v1 行为，开关 kv `zcode_auto_claim` **默认关**）：
/// 每 15 分钟探测一轮，找到可领的就领第一个——领取会开浏览器过验证码，
/// 所以必须是用户主动打开的开关。
fn spawn_zcode_auto_claim(
    db: Arc<Db>,
    service: Arc<crate::app::checkin::Service>,
    app: tauri::AppHandle,
) {
    use tauri::Emitter;

    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
            let enabled = crate::app::runtime::blocking({
                let db = db.clone();
                move || {
                    db.kv_get("zcode_auto_claim")
                        .ok()
                        .flatten()
                        .and_then(|v| serde_json::from_str::<bool>(&v).ok())
                        .unwrap_or(false)
                }
            })
            .await;
            // panic / 读库失败都按「开关关闭」处理——这个开关本来就是缺省关
            if !enabled.unwrap_or(false) {
                continue;
            }
            if let Some(line) = service.zcode_auto_claim_round().await {
                log::log(Level::Info, "checkin", format!("ZCode 自动领取：{line}"));
                let _ = app.emit("zcode:claim", &line);
            }
        }
    });
}

/// 任务与日志的事件桥（§6.3）：TaskStore 的广播 → `task:state` / `task:progress`，
/// log 的实时行 → `log:line`。进度单独成一个事件，界面的进度条只订阅它，
/// 状态变化不会把进度条重画一遍。
fn spawn_event_bridges(app: tauri::AppHandle, tasks: Arc<TaskStore>) {
    use std::collections::HashMap;
    use tauri::Emitter;

    let log_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut rx = tasks.subscribe();
        let mut last: HashMap<u64, (TaskState, Option<f32>)> = HashMap::new();
        loop {
            let Ok(view) = rx.recv().await else { break };
            let prev = last.insert(view.id, (view.state, view.progress));
            let state_changed = prev.map(|(s, _)| s != view.state).unwrap_or(true);
            if state_changed {
                let _ = app.emit("task:state", &view);
            } else if prev.map(|(_, p)| p != view.progress).unwrap_or(false) {
                let _ = app.emit("task:progress", &view);
            }
        }
    });

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
    let reload = MenuItem::with_id(app, "reload", "重载界面", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &reload, &quit])?;

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
            "quit" => app.exit(0),
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
