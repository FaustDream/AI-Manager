// system 的 IPC 命令：健康报告、诊断包、更新检测、配置快照。
// 全部只做转接，业务在 app/health、app/update、app/config_history。

use super::*;
use crate::app::error::ErrorKind;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

/// 上次提醒的签名与时间：同一批问题 24 小时内只弹一次，换了问题立刻弹。
const NOTIFY_KV: &str = "alert.notified";

/// 健康报告。顺带把待处理条数写到托盘提示上——
/// 窗口最小化时，托盘是唯一还能看见「有几件事要处理」的地方。
#[tauri::command]
pub async fn system_health(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::health::HealthReport, String> {
    let db = state.db.clone();
    let report = crate::app::runtime::blocking(move || crate::app::health::report(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;

    notify_alerts(&app, &report).await;

    if let Some(tray) = app.tray_by_id("main-tray") {
        let text = if report.alerts.is_empty() {
            format!("AI Manager v{} · 一切正常", report.version)
        } else {
            format!("AI Manager · {} 条待处理", report.alerts.len())
        };
        let _ = tray.set_tooltip(Some(text));
    }
    Ok(report)
}

/// 导出诊断包，返回文件路径。前端只负责提示「在哪」。
#[tauri::command]
pub async fn diagnose_bundle(state: State<'_, AppState>) -> std::result::Result<String, String> {
    let db = state.db.clone();
    let path = crate::app::runtime::blocking(move || crate::app::health::export_bundle(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;
    log::action(
        "diagnose_bundle",
        "user",
        true,
        0,
        format!("诊断包已导出：{}", path.display()),
    );
    Ok(path.display().to_string())
}

/// 检查更新：只查 GitHub Releases，不下载不替换。
#[tauri::command]
pub async fn update_check() -> std::result::Result<crate::app::update::UpdateInfo, String> {
    let http = crate::app::net::RealHttp::new();
    crate::app::update::check(&http, env!("CARGO_PKG_VERSION"))
        .await
        .map_err(err_text)
}

#[tauri::command]
pub async fn config_snapshot_list(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<crate::app::config_history::Snapshot>, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::config_history::list(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 回滚一条配置快照。`schedules` 回滚后要唤醒调度循环，否则新规则要等重启才生效。
#[tauri::command]
pub async fn config_snapshot_restore(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<crate::app::config_history::Snapshot, String> {
    let db = state.db.clone();
    let restored =
        crate::app::runtime::blocking(move || crate::app::config_history::restore(&db, &id))
            .await
            .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
            .map_err(err_text)?;
    if restored.key == "schedules" {
        crate::app::schedule::notify_changed();
    }
    log::action(
        "config_restore",
        "user",
        true,
        0,
        format!("配置已回滚：{}", restored.key),
    );
    Ok(restored)
}

#[tauri::command]
pub async fn config_snapshot_clear(state: State<'_, AppState>) -> std::result::Result<(), String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::config_history::clear(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 系统通知：只在「问题集合变了」或「距上次提醒超过 24 小时」时弹一条。
///
/// 每分钟轮询都弹一次等于训练用户忽略通知；而完全不弹，窗口关着时就永远不知道。
async fn notify_alerts(app: &tauri::AppHandle, report: &crate::app::health::HealthReport) {
    if report.alerts.is_empty() {
        return;
    }
    let mut titles: Vec<String> = report
        .alerts
        .iter()
        .map(|alert| format!("{}:{}", alert.level, alert.title))
        .collect();
    titles.sort();
    let signature = titles.join("|");
    let now = crate::app::timeutil::now_ms();

    let db = app.state::<AppState>().db.clone();
    let sig = signature.clone();
    let should = crate::app::runtime::blocking(move || {
        let previous: Option<serde_json::Value> = match db.kv_get(NOTIFY_KV) {
            Ok(Some(text)) => serde_json::from_str(&text).ok(),
            _ => None,
        };
        let same = previous
            .as_ref()
            .and_then(|value| value.get("sig"))
            .and_then(|value| value.as_str())
            == Some(sig.as_str());
        let at = previous
            .as_ref()
            .and_then(|value| value.get("at"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if same && now - at < 24 * 60 * 60 * 1000 {
            return Ok::<bool, AppError>(false);
        }
        db.kv_set(
            NOTIFY_KV,
            &serde_json::json!({ "sig": sig, "at": now }).to_string(),
        )?;
        Ok(true)
    })
    .await
    .ok()
    .and_then(|inner| inner.ok())
    .unwrap_or(false);

    if !should {
        return;
    }
    let body = report
        .alerts
        .iter()
        .take(3)
        .map(|alert| alert.title.clone())
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(e) = app
        .notification()
        .builder()
        .title(format!("AI Manager：{} 条待处理", report.alerts.len()))
        .body(body)
        .show()
    {
        // 通知发不出去不是致命错误（托盘提示与页面徽标仍在），但要留痕：
        // 否则「明明有告警却没弹」会变成查不出的玄学
        log::log(Level::Warn, "app", format!("系统通知发送失败：{e}"));
    }
}

/// 把一条告警标记为「已处理」：默认静默 24 小时，到期自动恢复提醒。
///
/// 只静默这一条（按 `key`），不改变判定逻辑——数据变好之后告警本来就会自己消失，
/// 这里是给「暂时不打算处理」留的出口。
#[tauri::command]
pub async fn alert_mute(
    state: State<'_, AppState>,
    key: String,
    hours: Option<i64>,
) -> std::result::Result<(), String> {
    let db = state.db.clone();
    let hours = hours.unwrap_or(24);
    let log_key = key.clone();
    crate::app::runtime::blocking(move || crate::app::health::mute(&db, &key, hours))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)?;
    log::action(
        "alert_mute",
        "user",
        true,
        0,
        format!("告警已处理：{log_key}"),
    );
    Ok(())
}

#[tauri::command]
pub async fn alert_unmute_all(state: State<'_, AppState>) -> std::result::Result<(), String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::health::unmute_all(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}
