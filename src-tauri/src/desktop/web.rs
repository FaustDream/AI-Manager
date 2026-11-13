//! 网页调试壳：把桌面壳的同一套命令面挂成本机 HTTP，前端在浏览器里直连。
//!
//! - `POST /cmd/<命令名>`：与 Tauri invoke 同语义（成功 `{"ok":true,"data":…}`，
//!   失败 `{"ok":false,"error":…}`，error 就是前端 `errText` 认识的那串 JSON）。
//! - `GET /events`：SSE，转发 `app::events` 统一事件总线（与桌面壳共用一条路径）。
//!
//! 命令复用的诀窍：壳进程持有一个**从不 run()** 的真实 Tauri 应用（不建窗口、
//! 不起事件循环），借它的 Manager 语义把 AppState 变成命令函数认识的
//! `State`——命令函数**原样**被调，不存在第二套转接逻辑。
//! 名单与 `generate_handler!` 一一对应：改命令两处一起改。
//!
//! 安全面：只绑 127.0.0.1；`/cmd` 必须带自定义头 `x-ai-manager-web`（强制浏览器
//! 跨站请求先走预检，而本服务不回预检——任何网页都发不起 CSRF）；带 Origin 时
//! 只认本机 dev server。curl 等本机工具不带 Origin，放行（本机进程本就在边界内）。

use crate::app::error::{AppError, ErrorKind};
use crate::desktop::AppState;
use serde_json::Value;
use tauri::Manager;

/// HTTP 层。运行时由调用方给定（bin 用真实 Wry 并在主线程 build，
/// 集成测试用 MockRuntime）；App 本体随 serve 的 future 走 block_on
/// 的线程，axum 状态里只放 Send + Sync 的 AppHandle。
mod http {
    use super::{args_error, dispatch};
    use crate::desktop::AppState;
    use axum::extract::State as AxumState;
    use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tauri::Manager;

    /// 网页壳的命令通道端口。5273 是 vite dev server，这里错开一位；
    /// vite.config.ts 的 proxy 与本常量必须成对改。bin 入口用它绑定。
    pub const WEB_PORT: u16 = 5274;

    /// listener 与预构建的 App 都由调用方传入：bin 用固定端口 + 真实 Wry
    /// （便于给占用提示；tao 要求事件循环建在主线程），测试用临时端口 +
    /// MockRuntime 做全链路验证。
    pub async fn serve<R: tauri::Runtime>(
        listener: tokio::net::TcpListener,
        app: tauri::App<R>,
        state: AppState,
    ) -> std::io::Result<()> {
        app.manage(state);
        let handle: Arc<tauri::AppHandle<R>> = Arc::new(app.app_handle().clone());

        let router = Router::new()
            .route("/cmd/{name}", post(cmd_entry))
            .route("/events", get(events_sse))
            .layer(axum::middleware::from_fn(security_guard))
            .with_state(handle);

        let addr = listener.local_addr()?;
        // 终端提示由 bin 入口负责（契约 19：desktop 层不写 stdout），这里只进日志
        crate::app::log::log(
            crate::app::log::Level::Info,
            "app",
            format!("网页调试壳就绪：http://{addr}（/cmd + /events）"),
        );
        axum::serve(listener, router).await
    }

    /// 安全面：见模块注释。只拦「浏览器身份可疑」的请求，不拦本机工具。
    async fn security_guard(
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        use axum::http::{header, Method, StatusCode};

        if let Some(origin) = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
        {
            let trusted = matches!(origin, "http://127.0.0.1:5273" | "http://localhost:5273");
            if !trusted {
                return (StatusCode::FORBIDDEN, "Origin 不受信任").into_response();
            }
        }
        if req.method() == Method::POST {
            let marked = req
                .headers()
                .get("x-ai-manager-web")
                .and_then(|v| v.to_str().ok())
                == Some("1");
            if !marked {
                return (StatusCode::FORBIDDEN, "缺少 x-ai-manager-web 请求头").into_response();
            }
        }
        next.run(req).await
    }

    async fn cmd_entry<R: tauri::Runtime>(
        AxumState(app): AxumState<Arc<tauri::AppHandle<R>>>,
        axum::extract::Path(name): axum::extract::Path<String>,
        body: axum::body::Bytes,
    ) -> Json<Value> {
        let v: Value = if body.is_empty() {
            json!({})
        } else {
            match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    return Json(json!({
                        "ok": false,
                        "error": args_error(&format!("请求体不是合法 JSON：{e}"))
                    }))
                }
            }
        };
        let reply = match dispatch(&app, &name, &v).await {
            Ok(data) => json!({ "ok": true, "data": data }),
            Err(error) => json!({ "ok": false, "error": error }),
        };
        Json(reply)
    }

    /// SSE 事件流。Lagged（订阅端跟不上）直接跳过——事件只是刷新提示。
    async fn events_sse(
    ) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
        use tokio_stream::StreamExt as _;
        let stream = tokio_stream::wrappers::BroadcastStream::new(crate::app::events::subscribe())
            .filter_map(|item| match item {
                Ok(event) => Some(Ok(SseEvent::default()
                    .event(event.name)
                    .data(event.payload.to_string()))),
                Err(_) => None,
            });
        Sse::new(stream).keep_alive(KeepAlive::default())
    }
}

pub use http::{serve, WEB_PORT};

fn args_error(message: &str) -> String {
    super::err_text(AppError::new(ErrorKind::Invalid, message.to_string()))
}

/// 命令结果统一转 JSON：Result 在这里**解包成裸载荷**——invoke 的语义是
/// 「成功给载荷、失败 reject」，整包序列化会产出 `{"Ok":…}`/`{"Err":…}` 壳，
/// 前端拿壳当数据用轻则形状不符（rows.reduce 崩溃），重则失败被当成成功。
/// 序列化失败按 Io 报——载荷类型都是 ts-rs 生成的，正常不会走到这一支。
fn to_json<T: serde::Serialize>(value: Result<T, String>) -> Result<Value, String> {
    value.and_then(|v| {
        serde_json::to_value(v).map_err(|e| {
            super::err_text(AppError::new(ErrorKind::Io, format!("结果序列化失败：{e}")))
        })
    })
}

/// 按参数名取值并反序列化。缺键视为 null：`Option<T>` 得到 None，
/// 非 Option 缺参得到明确的参数错误（与 invoke 的行为对齐）。
fn arg<T: serde::de::DeserializeOwned>(v: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(v.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|e| args_error(&format!("参数 {key} 不合法：{e}")))
}

/// 命令分发：名单与 `desktop/mod.rs` 的 `generate_handler!` 一一对应。
/// 每条命令一行转接，业务都在 `app/`——与桌面壳的命令文件同一套函数。
/// 对运行时泛型：serve 用真实 Wry 的 handle，集成测试（tests/web.rs）用 MockRuntime。
pub async fn dispatch<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    name: &str,
    v: &Value,
) -> Result<Value, String> {
    use crate::desktop::commands as cmd;

    match name {
        // ------------------------------------------------------------ shell
        // 这批命令不会失败：包一层 Ok 对齐 to_json 的 Result 形状
        "ui_ready" => to_json(Ok(cmd::shell::ui_ready(app.state::<AppState>()))),
        "app_version" => to_json(Ok(cmd::shell::app_version())),
        // 网页模式没有窗口与托盘：这两个命令留空壳，前端调用不报错
        "app_quit" => {
            crate::app::log::flush();
            Ok(Value::Null)
        }
        "app_show_window" => Ok(Value::Null),
        "open_external" => {
            let url = arg(v, "url")?;
            to_json(cmd::shell::open_external(url))
        }
        "open_private_browser" => {
            let url = arg(v, "url")?;
            to_json(cmd::shell::open_private_browser(url))
        }

        // ------------------------------------------------------------ settings
        "settings_get" => {
            let key = arg(v, "key")?;
            to_json(cmd::settings::settings_get(app.state::<AppState>(), key).await)
        }
        "settings_set" => {
            let (key, value) = (arg(v, "key")?, arg(v, "value")?);
            to_json(cmd::settings::settings_set(app.state::<AppState>(), key, value).await)
        }
        "config_get" => to_json(Ok(cmd::settings::config_get())),
        "config_set" => {
            let config = arg(v, "config")?;
            to_json(cmd::settings::config_set(config))
        }
        "network_selfcheck" => to_json(cmd::settings::network_selfcheck().await),
        "autostart_get" => to_json(Ok(cmd::settings::autostart_get())),
        "autostart_set" => {
            let enabled = arg(v, "enabled")?;
            to_json(cmd::settings::autostart_set(enabled))
        }
        "log_level" => {
            let level = arg(v, "level")?;
            to_json(cmd::settings::log_level(level))
        }

        // ------------------------------------------------------------ accounts
        "account_list" => to_json(cmd::accounts::account_list(app.state::<AppState>()).await),
        "account_set_alias" => {
            let (iid, alias) = (arg(v, "iid")?, arg(v, "alias")?);
            to_json(cmd::accounts::account_set_alias(app.state::<AppState>(), iid, alias).await)
        }
        "account_reward_credits" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_reward_credits(app.state::<AppState>(), iid).await)
        }
        "account_set_enabled" => {
            let (iid, enabled) = (arg(v, "iid")?, arg(v, "enabled")?);
            to_json(cmd::accounts::account_set_enabled(app.state::<AppState>(), iid, enabled).await)
        }
        "account_reorder" => {
            let iids = arg(v, "iids")?;
            to_json(cmd::accounts::account_reorder(app.state::<AppState>(), iids).await)
        }
        "account_delete" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_delete(app.state::<AppState>(), iid).await)
        }
        "account_scan" => to_json(cmd::accounts::account_scan(app.state::<AppState>()).await),
        "account_add_cookie" => {
            let (platform, cookie, name) =
                (arg(v, "platform")?, arg(v, "cookie")?, arg(v, "name")?);
            to_json(
                cmd::accounts::account_add_cookie(app.state::<AppState>(), platform, cookie, name)
                    .await,
            )
        }
        "account_login_start" => {
            let platform = arg(v, "platform")?;
            to_json(cmd::accounts::account_login_start(app.state::<AppState>(), platform).await)
        }
        "account_relogin_start" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_relogin_start(app.state::<AppState>(), iid).await)
        }
        "account_refresh_all" => {
            to_json(cmd::accounts::account_refresh_all(app.state::<AppState>()).await)
        }
        "account_refresh_credential" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_refresh_credential(app.state::<AppState>(), iid).await)
        }
        "account_refresh_credentials" => {
            let iids = arg(v, "iids")?;
            to_json(cmd::accounts::account_refresh_credentials(app.state::<AppState>(), iids).await)
        }
        "account_switch" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_switch(app.state::<AppState>(), iid).await)
        }
        "account_switch_codebuddy" => {
            let (iid, target) = (arg(v, "iid")?, arg(v, "target")?);
            to_json(
                cmd::accounts::account_switch_codebuddy(app.state::<AppState>(), iid, target).await,
            )
        }
        "account_codebuddy_targets" => {
            to_json(cmd::accounts::account_codebuddy_targets(app.state::<AppState>()).await)
        }
        "account_restore_sessions" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_restore_sessions(app.state::<AppState>(), iid).await)
        }
        "account_uid" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_uid(app.state::<AppState>(), iid).await)
        }
        "account_current" => to_json(cmd::accounts::account_current().await),
        "account_add_prepare" => {
            let platform = arg(v, "platform")?;
            to_json(cmd::accounts::account_add_prepare(platform).await)
        }
        "account_open_client" => {
            let platform = arg(v, "platform")?;
            to_json(cmd::accounts::account_open_client(platform).await)
        }
        "account_set_note" => {
            let (iid, note) = (arg(v, "iid")?, arg(v, "note")?);
            to_json(cmd::accounts::account_set_note(app.state::<AppState>(), iid, note).await)
        }
        "account_batch_diagnose" => {
            let iids = arg(v, "iids")?;
            to_json(cmd::accounts::account_batch_diagnose(app.state::<AppState>(), iids).await)
        }
        "account_export" => to_json(cmd::accounts::account_export(app.state::<AppState>()).await),

        // ------------------------------------------------------------ hub
        "hub_snapshot" => to_json(cmd::hub::hub_snapshot(app.state::<AppState>()).await),
        "hub_skill_import_many" => {
            let (target, names) = (arg(v, "target")?, arg(v, "names")?);
            to_json(cmd::hub::hub_skill_import_many(app.state::<AppState>(), target, names).await)
        }
        "hub_skill_import_local" => {
            let path = arg(v, "path")?;
            to_json(cmd::hub::hub_skill_import_local(app.state::<AppState>(), path).await)
        }
        "hub_skill_create" => {
            let (name, text) = (arg(v, "name")?, arg(v, "text")?);
            to_json(cmd::hub::hub_skill_create(app.state::<AppState>(), name, text).await)
        }
        "hub_mcp_claim" => {
            let (name, source, targets) = (arg(v, "name")?, arg(v, "source")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_mcp_claim(app.state::<AppState>(), name, source, targets).await)
        }
        "hub_skill_deploy" => {
            let (name, targets, overwrite) =
                (arg(v, "name")?, arg(v, "targets")?, arg(v, "overwrite")?);
            to_json(
                cmd::hub::hub_skill_deploy(app.state::<AppState>(), name, targets, overwrite).await,
            )
        }
        "hub_skill_remove" => {
            let (name, targets) = (arg(v, "name")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_skill_remove(app.state::<AppState>(), name, targets).await)
        }
        "hub_skill_import" => {
            let (target, name) = (arg(v, "target")?, arg(v, "name")?);
            to_json(cmd::hub::hub_skill_import(app.state::<AppState>(), target, name).await)
        }
        "hub_mcp_import" => {
            let (text, targets) = (arg(v, "text")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_mcp_import(app.state::<AppState>(), text, targets).await)
        }
        "hub_mcp_remove" => {
            let (name, targets) = (arg(v, "name")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_mcp_remove(app.state::<AppState>(), name, targets).await)
        }
        "hub_mcp_deploy" => {
            let (name, targets) = (arg(v, "name")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_mcp_deploy(app.state::<AppState>(), name, targets).await)
        }

        // ------------------------------------------------------------ sessions
        "session_scan" => to_json(cmd::sessions::session_scan(app.state::<AppState>()).await),
        "session_page" => {
            let (cursor, limit, filter) = (arg(v, "cursor")?, arg(v, "limit")?, arg(v, "filter")?);
            to_json(
                cmd::sessions::session_page(app.state::<AppState>(), cursor, limit, filter).await,
            )
        }
        "session_preview" => {
            let key = arg(v, "key")?;
            to_json(cmd::sessions::session_preview(app.state::<AppState>(), key).await)
        }
        "session_running_clients" => {
            let keys = arg(v, "keys")?;
            to_json(cmd::sessions::session_running_clients(keys).await)
        }
        "session_keys" => {
            let filter = arg(v, "filter")?;
            to_json(cmd::sessions::session_keys(app.state::<AppState>(), filter).await)
        }
        "session_facets" => to_json(cmd::sessions::session_facets(app.state::<AppState>()).await),
        "session_archive" => {
            let (keys, on) = (arg(v, "keys")?, arg(v, "on")?);
            to_json(cmd::sessions::session_archive(app.state::<AppState>(), keys, on).await)
        }
        "session_purge" => {
            let keys = arg(v, "keys")?;
            to_json(cmd::sessions::session_purge(app.state::<AppState>(), keys).await)
        }
        "session_retention" => {
            to_json(cmd::sessions::session_retention(app.state::<AppState>()).await)
        }
        "session_retention_set" => {
            let days = arg(v, "days")?;
            to_json(cmd::sessions::session_retention_set(app.state::<AppState>(), days).await)
        }

        // ------------------------------------------------------------ activity
        "activity_overview" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::activity::activity_overview(app.state::<AppState>(), iid).await)
        }
        "activity_claim" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::activity::activity_claim(app.state::<AppState>(), iid).await)
        }
        "activity_depart" => {
            let (iid, location) = (arg(v, "iid")?, arg(v, "location")?);
            to_json(cmd::activity::activity_depart(app.state::<AppState>(), iid, location).await)
        }
        "activity_daily_status" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::activity::activity_daily_status(app.state::<AppState>(), iid).await)
        }
        "activity_pref_set" => {
            let (iid, enabled, location) =
                (arg(v, "iid")?, arg(v, "enabled")?, arg(v, "location")?);
            to_json(
                cmd::activity::activity_pref_set(app.state::<AppState>(), iid, enabled, location)
                    .await,
            )
        }

        // ------------------------------------------------------------ checkin
        "checkin_credits" => {
            let (iids, trigger) = (arg(v, "iids")?, arg(v, "trigger")?);
            to_json(cmd::checkin::checkin_credits(app.state::<AppState>(), iids, trigger).await)
        }
        "checkin_diagnose" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::checkin::checkin_diagnose(app.state::<AppState>(), iid).await)
        }
        "checkin_detail" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::checkin::checkin_detail(app.state::<AppState>(), iid).await)
        }
        "checkin_run" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::checkin::checkin_run(app.state::<AppState>(), iid).await)
        }
        "checkin_batch" => {
            let iids = arg(v, "iids")?;
            to_json(cmd::checkin::checkin_batch(app.state::<AppState>(), iids).await)
        }
        "daily_state" => to_json(Ok(cmd::checkin::daily_state(app.state::<AppState>()))),
        "daily_run_now" => to_json(cmd::checkin::daily_run_now(app.state::<AppState>()).await),
        "zcode_claim_plans" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::checkin::zcode_claim_plans(app.state::<AppState>(), iid).await)
        }
        "zcode_claim_run" => {
            let (iid, plan_id) = (arg(v, "iid")?, arg(v, "plan_id")?);
            to_json(cmd::checkin::zcode_claim_run(app.state::<AppState>(), iid, plan_id).await)
        }
        "credit_trend" => {
            let (iid, days) = (arg(v, "iid")?, arg(v, "days")?);
            to_json(cmd::checkin::credit_trend(app.state::<AppState>(), iid, days).await)
        }
        "credit_claims_recent" => {
            let limit = arg(v, "limit")?;
            to_json(cmd::checkin::credit_claims_recent(app.state::<AppState>(), limit).await)
        }
        "credit_flow" => {
            let days = arg(v, "days")?;
            to_json(cmd::checkin::credit_flow(app.state::<AppState>(), days).await)
        }

        // ------------------------------------------------------------ schedule
        "schedule_list" => to_json(cmd::schedule::schedule_list(app.state::<AppState>()).await),
        "schedule_save" => {
            let input = arg(v, "input")?;
            to_json(cmd::schedule::schedule_save(app.state::<AppState>(), input).await)
        }
        "schedule_delete" => {
            let id = arg(v, "id")?;
            to_json(cmd::schedule::schedule_delete(app.state::<AppState>(), id).await)
        }
        "schedule_run" => {
            let id = arg(v, "id")?;
            to_json(cmd::schedule::schedule_run(app.state::<AppState>(), id).await)
        }

        // ------------------------------------------------------------ logs
        "log_query" => {
            let filter = arg(v, "filter")?;
            to_json(cmd::logs::log_query(filter).await)
        }
        "log_action" => {
            let (action, trigger, ok, ms, detail) = (
                arg(v, "action")?,
                arg(v, "trigger")?,
                arg(v, "ok")?,
                arg(v, "ms")?,
                arg(v, "detail")?,
            );
            to_json(cmd::logs::log_action(action, trigger, ok, ms, detail))
        }
        "log_ui" => {
            let (level, action, message, evidence) = (
                arg(v, "level")?,
                arg(v, "action")?,
                arg(v, "message")?,
                arg(v, "evidence")?,
            );
            to_json(cmd::logs::log_ui(level, action, message, evidence))
        }
        "log_span" => to_json(cmd::logs::log_span().await),
        "log_clear" => to_json(cmd::logs::log_clear()),
        "log_export" => {
            let (path, filter) = (arg(v, "path")?, arg(v, "filter")?);
            to_json(cmd::logs::log_export(path, filter).await)
        }
        "log_stats" => {
            let filter = arg(v, "filter")?;
            to_json(cmd::logs::log_stats(filter).await)
        }

        // ------------------------------------------------------------ system
        // system_health 带 AppHandle<Wry>（托盘提示），mock 运行时调不动；
        // 网页模式没有托盘，直接走同一个业务函数。
        "system_health" => {
            let db = app.state::<AppState>().db.clone();
            to_json(
                crate::app::runtime::blocking(move || crate::app::health::report(&db))
                    .await
                    .map_err(|e| super::err_text(AppError::new(ErrorKind::Io, e.to_string())))
                    .and_then(|r| r.map_err(super::err_text)),
            )
        }
        "diagnose_bundle" => to_json(cmd::system::diagnose_bundle(app.state::<AppState>()).await),
        "platform_capabilities" => to_json(Ok(cmd::system::platform_capabilities())),
        "update_check" => to_json(cmd::system::update_check().await),
        "config_snapshot_list" => {
            to_json(cmd::system::config_snapshot_list(app.state::<AppState>()).await)
        }
        "config_snapshot_restore" => {
            let id = arg(v, "id")?;
            to_json(cmd::system::config_snapshot_restore(app.state::<AppState>(), id).await)
        }
        "config_snapshot_clear" => {
            to_json(cmd::system::config_snapshot_clear(app.state::<AppState>()).await)
        }

        // ------------------------------------------------------------ usage
        "usage_scan" => to_json(cmd::usage::usage_scan(app.state::<AppState>()).await),
        "usage_overview" => {
            let days = arg(v, "days")?;
            to_json(cmd::usage::usage_overview(app.state::<AppState>(), days).await)
        }

        // ------------------------------------------------------------ maintenance
        "maintenance_space" => to_json(Ok(cmd::maintenance::maintenance_space().await)),
        "maintenance_trash" => to_json(Ok(cmd::maintenance::maintenance_trash().await)),
        "maintenance_purge_trash" => {
            let tag = arg(v, "tag")?;
            to_json(cmd::maintenance::maintenance_purge_trash(tag).await)
        }
        "maintenance_open_data" => to_json(cmd::maintenance::maintenance_open_data()),

        _ => Err(args_error(&format!("未知命令：{name}"))),
    }
}
