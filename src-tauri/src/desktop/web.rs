//! HTTP 壳：把命令面挂成本机 HTTP，并托管前端静态产物，浏览器是唯一界面。
//!
//! - `POST /cmd/<命令名>`：成功 `{"ok":true,"data":…}`，失败 `{"ok":false,"error":…}`，
//!   error 就是前端 `errText` 认识的那串 JSON。
//! - `GET /events`：SSE，转发 `app::events` 统一事件总线。
//! - 其余 GET：托管 `paths::web_dist()` 下的前端产物（vite build 输出），
//!   找不到文件回落 index.html（SPA 路由）。单端口同源，无需 CORS。
//!
//! 命令函数直接收 `&AppState`，不存在第二套转接逻辑。
//! 分发表是命令名单的唯一登记处：改命令同步改这里（D8）。
//!
//! 安全面：只绑 127.0.0.1；`/cmd` 必须带自定义头 `x-ai-manager-web`（强制浏览器
//! 跨站请求先走预检，而本服务不回预检——任何网页都发不起 CSRF）；带 Origin 时
//! 只认本机 dev server（5273）与本服务自身（5274，同源页面的 POST 会带它）。
//! curl 等本机工具不带 Origin，放行（本机进程本就在边界内）。

use crate::app::error::{AppError, ErrorKind};
use crate::desktop::AppState;
use serde_json::Value;

/// HTTP 层。AppState 直接进 axum 状态，命令函数收 &AppState。
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

    /// 命令通道端口。5273 是 vite dev server，这里错开一位；
    /// vite.config.ts 的 proxy 与本常量必须成对改。bin 入口用它绑定。
    pub const WEB_PORT: u16 = 5274;

    /// listener 由调用方传入：bin 用固定端口（便于给占用提示），
    /// 测试用临时端口做全链路验证。
    pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
        let dist = crate::app::paths::web_dist();
        let router = Router::new()
            .route("/cmd/{name}", post(cmd_entry))
            .route("/events", get(events_sse))
            .layer(axum::middleware::from_fn(security_guard))
            .fallback(static_entry)
            .with_state((Arc::new(state), Arc::new(dist)));

        let addr = listener.local_addr()?;
        // 终端提示由 bin 入口负责（契约 19：壳层不写 stdout），这里只进日志
        crate::app::log::log(
            crate::app::log::Level::Info,
            "app",
            format!("HTTP 壳就绪：http://{addr}（页面 + /cmd + /events）"),
        );
        axum::serve(listener, router).await
    }

    /// 安全面：见模块注释。只拦「浏览器身份可疑」的请求，不拦本机工具。
    /// 只作用于 /cmd 与 /events；静态页面是普通 GET 导航，不带这些头。
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
            let trusted = matches!(
                origin,
                "http://127.0.0.1:5273"
                    | "http://localhost:5273"
                    | "http://127.0.0.1:5274"
                    | "http://localhost:5274"
            );
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

    async fn cmd_entry(
        AxumState((state, _dist)): AxumState<(Arc<AppState>, Arc<std::path::PathBuf>)>,
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
        let reply = match dispatch(&state, &name, &v).await {
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

    /// 静态产物托管：GET 命中文件给文件，其余回落 index.html（SPA 路由）。
    /// 不引 tower-http：就这一条回落逻辑，几十行直白代码比一层抽象更合适。
    async fn static_entry(
        AxumState((_state, dist)): AxumState<(Arc<AppState>, Arc<std::path::PathBuf>)>,
        req: axum::extract::Request,
    ) -> axum::response::Response {
        use axum::http::{header, Method, StatusCode};

        if req.method() != Method::GET && req.method() != Method::HEAD {
            return (StatusCode::METHOD_NOT_ALLOWED, "只接受 GET").into_response();
        }
        let raw = req.uri().path().trim_start_matches('/');
        // 路径穿越防线：拒绝 ..、盘符与反斜杠。正常产物路径只有字母数字与 -._/
        let safe =
            !raw.is_empty() && !raw.contains("..") && !raw.contains(':') && !raw.contains('\\');
        let target = if safe {
            dist.join(raw)
        } else {
            dist.join("index.html")
        };
        let target = if target.is_file() {
            target
        } else {
            dist.join("index.html")
        };
        match tokio::fs::read(&target).await {
            Ok(bytes) => {
                let mime = mime_of(&target);
                let mut resp = ([(header::CONTENT_TYPE, mime)], bytes).into_response();
                // index.html 不许缓存：前端文件夹覆盖更新后，刷新必须拿到新入口
                if target.file_name().and_then(|n| n.to_str()) == Some("index.html") {
                    resp.headers_mut()
                        .insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
                }
                resp
            }
            Err(_) => (
                StatusCode::NOT_FOUND,
                "前端产物缺失：先执行 npm run build（dist/ 是托管的输入）",
            )
                .into_response(),
        }
    }

    /// 扩展名 → Content-Type。只列 vite 产物实际会出现的几类。
    fn mime_of(path: &std::path::Path) -> &'static str {
        match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "html" => "text/html; charset=utf-8",
            "js" | "mjs" => "text/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "json" | "map" => "application/json; charset=utf-8",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "ico" => "image/x-icon",
            "woff2" => "font/woff2",
            "woff" => "font/woff",
            "ttf" => "font/ttf",
            "txt" => "text/plain; charset=utf-8",
            "wasm" => "application/wasm",
            _ => "application/octet-stream",
        }
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

/// 命令分发：本表是命令名单的唯一登记处（D8），与前端 `cmd.ts` 的
/// `CmdReturn` 一一对应（tests/command_surface.rs 钉死这份一致性）。
/// 每条命令一行转接，业务都在 `app/`。
pub async fn dispatch(state: &AppState, name: &str, v: &Value) -> Result<Value, String> {
    use crate::desktop::commands as cmd;

    match name {
        // ------------------------------------------------------------ shell
        // 这批命令不会失败：包一层 Ok 对齐 to_json 的 Result 形状
        "ui_ready" => to_json(Ok(cmd::shell::ui_ready(state))),
        "app_version" => to_json(Ok(cmd::shell::app_version())),
        // app_quit 真退出：先回包再延迟 exit（见命令注释），前端据此显示「已退出」
        "app_quit" => {
            cmd::shell::app_quit();
            Ok(Value::Null)
        }
        "app_show_window" => {
            cmd::shell::app_show_window();
            Ok(Value::Null)
        }
        "open_external" => {
            let url = arg(v, "url")?;
            to_json(cmd::shell::open_external(url))
        }
        "open_private_browser" => {
            let url = arg(v, "url")?;
            to_json(cmd::shell::open_private_browser(url))
        }
        "model_api_info" => to_json(cmd::shell::model_api_info(state).await),
        "model_api_start" => to_json(cmd::shell::model_api_start(state).await),
        "model_api_stop" => to_json(cmd::shell::model_api_stop(state).await),
        "model_api_restart" => to_json(cmd::shell::model_api_restart(state).await),
        "model_api_stats" => to_json(Ok(cmd::shell::model_api_stats())),
        "model_api_probe" => to_json(cmd::shell::model_api_probe().await),
        "model_api_models" => to_json(cmd::shell::model_api_models(state).await),
        "model_api_set_port" => {
            let port = arg(v, "port")?;
            to_json(cmd::shell::model_api_set_port(state, port).await)
        }
        "model_api_regenerate_key" => to_json(cmd::shell::model_api_regenerate_key(state).await),

        // ------------------------------------------------------------ settings
        "settings_get" => {
            let key = arg(v, "key")?;
            to_json(cmd::settings::settings_get(state, key).await)
        }
        "settings_set" => {
            let (key, value) = (arg(v, "key")?, arg(v, "value")?);
            to_json(cmd::settings::settings_set(state, key, value).await)
        }
        "config_get" => to_json(Ok(cmd::settings::config_get())),
        "config_set" => {
            let config = arg(v, "config")?;
            to_json(cmd::settings::config_set(config))
        }
        "config_reset_builtin" => to_json(cmd::settings::config_reset_builtin()),
        "network_selfcheck" => to_json(cmd::settings::network_selfcheck().await),
        "log_level" => {
            let level = arg(v, "level")?;
            to_json(cmd::settings::log_level(level))
        }

        // ------------------------------------------------------------ accounts
        "account_list" => to_json(cmd::accounts::account_list(state).await),
        "account_set_alias" => {
            let (iid, alias) = (arg(v, "iid")?, arg(v, "alias")?);
            to_json(cmd::accounts::account_set_alias(state, iid, alias).await)
        }
        "account_reward_credits" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_reward_credits(state, iid).await)
        }
        "account_set_enabled" => {
            let (iid, enabled) = (arg(v, "iid")?, arg(v, "enabled")?);
            to_json(cmd::accounts::account_set_enabled(state, iid, enabled).await)
        }
        "account_reorder" => {
            let iids = arg(v, "iids")?;
            to_json(cmd::accounts::account_reorder(state, iids).await)
        }
        "account_delete" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_delete(state, iid).await)
        }
        "account_scan" => to_json(cmd::accounts::account_scan(state).await),
        "account_scan_platform" => {
            let platform = arg(v, "platform")?;
            to_json(cmd::accounts::account_scan_platform(state, platform).await)
        }
        // 豆包浏览器登录四件套：已登记在分发表里。
        "doubao_browser_list" => to_json(cmd::accounts::doubao_browser_list().await),
        "doubao_browser_login_start" => {
            let browser_id = arg(v, "browser_id")?;
            to_json(cmd::accounts::doubao_browser_login_start(state, browser_id).await)
        }
        "doubao_browser_login_status" => {
            to_json(cmd::accounts::doubao_browser_login_status().await)
        }
        "doubao_browser_login_cancel" => {
            to_json(cmd::accounts::doubao_browser_login_cancel().await)
        }
        "account_switchable" => to_json(cmd::accounts::account_switchable().await),
        "account_add_cookie" => {
            let (platform, cookie, name) =
                (arg(v, "platform")?, arg(v, "cookie")?, arg(v, "name")?);
            to_json(cmd::accounts::account_add_cookie(state, platform, cookie, name).await)
        }
        "account_login_start" => {
            let platform = arg(v, "platform")?;
            to_json(cmd::accounts::account_login_start(state, platform).await)
        }
        "account_reauth_start" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_reauth_start(state, iid).await)
        }
        "account_refresh_all" => to_json(cmd::accounts::account_refresh_all(state).await),
        "account_refresh_credential" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_refresh_credential(state, iid).await)
        }
        "account_refresh_credentials" => {
            let iids = arg(v, "iids")?;
            to_json(cmd::accounts::account_refresh_credentials(state, iids).await)
        }
        "account_switch" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_switch(state, iid).await)
        }
        "account_switch_codebuddy" => {
            let (iid, target) = (arg(v, "iid")?, arg(v, "target")?);
            to_json(cmd::accounts::account_switch_codebuddy(state, iid, target).await)
        }
        "account_codebuddy_targets" => {
            to_json(cmd::accounts::account_codebuddy_targets(state).await)
        }
        "account_restore_sessions" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_restore_sessions(state, iid).await)
        }
        "account_uid" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::accounts::account_uid(state, iid).await)
        }
        "account_current" => to_json(cmd::accounts::account_current().await),
        "account_add_finish" => {
            let platform = arg(v, "platform")?;
            to_json(cmd::accounts::account_add_finish(state, platform).await)
        }
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
            to_json(cmd::accounts::account_set_note(state, iid, note).await)
        }

        // ------------------------------------------------------------ hub
        "hub_snapshot" => to_json(cmd::hub::hub_snapshot(state).await),
        "hub_skill_import_many" => {
            let (target, names) = (arg(v, "target")?, arg(v, "names")?);
            to_json(cmd::hub::hub_skill_import_many(state, target, names).await)
        }
        "hub_skill_import_local" => {
            let path = arg(v, "path")?;
            to_json(cmd::hub::hub_skill_import_local(state, path).await)
        }
        "hub_skill_create" => {
            let (name, text) = (arg(v, "name")?, arg(v, "text")?);
            to_json(cmd::hub::hub_skill_create(state, name, text).await)
        }
        "hub_mcp_claim" => {
            let (name, source, targets) = (arg(v, "name")?, arg(v, "source")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_mcp_claim(state, name, source, targets).await)
        }
        "hub_skill_deploy" => {
            let (name, targets, overwrite) =
                (arg(v, "name")?, arg(v, "targets")?, arg(v, "overwrite")?);
            to_json(cmd::hub::hub_skill_deploy(state, name, targets, overwrite).await)
        }
        "hub_skill_remove" => {
            let (name, targets) = (arg(v, "name")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_skill_remove(state, name, targets).await)
        }
        "hub_skill_import" => {
            let (target, name) = (arg(v, "target")?, arg(v, "name")?);
            to_json(cmd::hub::hub_skill_import(state, target, name).await)
        }
        "hub_mcp_import" => {
            let (text, targets) = (arg(v, "text")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_mcp_import(state, text, targets).await)
        }
        "hub_mcp_remove" => {
            let (name, targets) = (arg(v, "name")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_mcp_remove(state, name, targets).await)
        }
        "hub_mcp_deploy" => {
            let (name, targets) = (arg(v, "name")?, arg(v, "targets")?);
            to_json(cmd::hub::hub_mcp_deploy(state, name, targets).await)
        }

        // ------------------------------------------------------------ sessions
        "session_scan" => to_json(cmd::sessions::session_scan(state).await),
        "session_page" => {
            let (cursor, limit, filter) = (arg(v, "cursor")?, arg(v, "limit")?, arg(v, "filter")?);
            to_json(cmd::sessions::session_page(state, cursor, limit, filter).await)
        }
        "session_preview" => {
            let key = arg(v, "key")?;
            to_json(cmd::sessions::session_preview(state, key).await)
        }
        "session_running_clients" => {
            let keys = arg(v, "keys")?;
            to_json(cmd::sessions::session_running_clients(keys).await)
        }
        "session_keys" => {
            let filter = arg(v, "filter")?;
            to_json(cmd::sessions::session_keys(state, filter).await)
        }
        "session_facets" => to_json(cmd::sessions::session_facets(state).await),
        "session_archive" => {
            let (keys, on) = (arg(v, "keys")?, arg(v, "on")?);
            to_json(cmd::sessions::session_archive(state, keys, on).await)
        }
        "session_purge" => {
            let keys = arg(v, "keys")?;
            to_json(cmd::sessions::session_purge(state, keys).await)
        }
        "session_retention" => to_json(cmd::sessions::session_retention(state).await),
        "session_retention_set" => {
            let days = arg(v, "days")?;
            to_json(cmd::sessions::session_retention_set(state, days).await)
        }
        "session_pin" => {
            let (keys, on) = (arg(v, "keys")?, arg(v, "on")?);
            to_json(cmd::sessions::session_pin(state, keys, on).await)
        }
        "session_rename" => {
            let (key, title) = (arg(v, "key")?, arg(v, "title")?);
            to_json(cmd::sessions::session_rename(state, key, title).await)
        }
        "session_hide" => {
            let (keys, on) = (arg(v, "keys")?, arg(v, "on")?);
            to_json(cmd::sessions::session_hide(state, keys, on).await)
        }
        "session_tag_set" => {
            let (key, tags) = (arg(v, "key")?, arg(v, "tags")?);
            to_json(cmd::sessions::session_tag_set(state, key, tags).await)
        }
        "session_tags_of" => {
            let key = arg(v, "key")?;
            to_json(cmd::sessions::session_tags_of(state, key).await)
        }
        "session_export" => {
            let (keys, format) = (arg(v, "keys")?, arg(v, "format")?);
            to_json(cmd::sessions::session_export(state, keys, format).await)
        }
        "session_open_body" => {
            let key = arg(v, "key")?;
            to_json(cmd::sessions::session_open_body(state, key).await)
        }
        "session_open_client" => {
            let key = arg(v, "key")?;
            to_json(cmd::sessions::session_open_client(key).await)
        }
        "session_suggest_titles" => {
            let keys = arg(v, "keys")?;
            to_json(cmd::sessions::session_suggest_titles(state, keys).await)
        }
        "session_stats" => {
            let (from, to) = (arg(v, "from")?, arg(v, "to")?);
            to_json(cmd::sessions::session_stats(state, from, to).await)
        }

        // ------------------------------------------------------------ activity
        "activity_overview" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::activity::activity_overview(state, iid).await)
        }
        "activity_claim" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::activity::activity_claim(state, iid).await)
        }
        "activity_depart" => {
            let (iid, location) = (arg(v, "iid")?, arg(v, "location")?);
            to_json(cmd::activity::activity_depart(state, iid, location).await)
        }
        "activity_daily_status" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::activity::activity_daily_status(state, iid).await)
        }
        "activity_pref_set" => {
            let (iid, enabled, location) =
                (arg(v, "iid")?, arg(v, "enabled")?, arg(v, "location")?);
            to_json(cmd::activity::activity_pref_set(state, iid, enabled, location).await)
        }

        // ------------------------------------------------------------ checkin
        "checkin_credits" => {
            let (iids, trigger) = (arg(v, "iids")?, arg(v, "trigger")?);
            to_json(cmd::checkin::checkin_credits(state, iids, trigger).await)
        }
        "checkin_refresh" => {
            let (iids, trigger) = (arg(v, "iids")?, arg(v, "trigger")?);
            to_json(cmd::checkin::checkin_refresh(state, iids, trigger).await)
        }
        "checkin_detail" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::checkin::checkin_detail(state, iid).await)
        }
        "checkin_run" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::checkin::checkin_run(state, iid).await)
        }
        "checkin_batch" => {
            let iids = arg(v, "iids")?;
            to_json(cmd::checkin::checkin_batch(state, iids).await)
        }
        "daily_state" => to_json(Ok(cmd::checkin::daily_state(state))),
        "daily_run_now" => to_json(cmd::checkin::daily_run_now(state).await),
        "zcode_claim_plans" => {
            let iid = arg(v, "iid")?;
            to_json(cmd::checkin::zcode_claim_plans(state, iid).await)
        }
        "zcode_claim_run" => {
            let (iid, plan_id) = (arg(v, "iid")?, arg(v, "plan_id")?);
            to_json(cmd::checkin::zcode_claim_run(state, iid, plan_id).await)
        }
        "credit_trend" => {
            let (iid, from, to) = (arg(v, "iid")?, arg(v, "from")?, arg(v, "to")?);
            to_json(cmd::checkin::credit_trend(state, iid, from, to).await)
        }
        "credit_claims_recent" => {
            let limit = arg(v, "limit")?;
            to_json(cmd::checkin::credit_claims_recent(state, limit).await)
        }
        "credit_flow" => {
            let (from, to) = (arg(v, "from")?, arg(v, "to")?);
            to_json(cmd::checkin::credit_flow(state, from, to).await)
        }

        // ------------------------------------------------------------ dida
        "dida_state" => to_json(Ok(cmd::dida::dida_state(state).await)),
        "dida_token_set" => {
            let token = arg(v, "token")?;
            to_json(cmd::dida::dida_token_set(state, token).await)
        }
        "dida_token_clear" => to_json(cmd::dida::dida_token_clear(state).await),
        "dida_task_create" => {
            let (title, project_id, content, due_date, priority, is_all_day) = (
                arg(v, "title")?,
                arg(v, "project_id")?,
                arg(v, "content")?,
                arg(v, "due_date")?,
                arg(v, "priority")?,
                arg(v, "is_all_day")?,
            );
            to_json(
                cmd::dida::dida_task_create(
                    state, title, project_id, content, due_date, priority, is_all_day,
                )
                .await,
            )
        }
        "dida_tasks" => {
            let project_id = arg(v, "project_id")?;
            to_json(cmd::dida::dida_tasks(state, project_id).await)
        }
        "dida_task_update" => {
            let (task_id, project_id, title, content, due_date, priority, is_all_day) = (
                arg(v, "task_id")?,
                arg(v, "project_id")?,
                arg(v, "title")?,
                arg(v, "content")?,
                arg(v, "due_date")?,
                arg(v, "priority")?,
                arg(v, "is_all_day")?,
            );
            to_json(
                cmd::dida::dida_task_update(
                    state, task_id, project_id, title, content, due_date, priority, is_all_day,
                )
                .await,
            )
        }
        "dida_task_complete" => {
            let (project_id, task_id) = (arg(v, "project_id")?, arg(v, "task_id")?);
            to_json(cmd::dida::dida_task_complete(state, project_id, task_id).await)
        }
        "dida_task_delete" => {
            let (project_id, task_id) = (arg(v, "project_id")?, arg(v, "task_id")?);
            to_json(cmd::dida::dida_task_delete(state, project_id, task_id).await)
        }
        "dida_task_save" => {
            let input = arg(v, "input")?;
            to_json(cmd::dida::dida_task_save(state, input).await)
        }
        "dida_task_detail" => {
            let (project_id, task_id) = (arg(v, "project_id")?, arg(v, "task_id")?);
            to_json(cmd::dida::dida_task_detail(state, project_id, task_id).await)
        }
        "dida_tasks_completed" => {
            let (project_ids, from, to) =
                (arg(v, "project_ids")?, arg(v, "from")?, arg(v, "to")?);
            to_json(cmd::dida::dida_tasks_completed(state, project_ids, from, to).await)
        }
        "dida_tasks_undone" => to_json(cmd::dida::dida_tasks_undone(state).await),
        "dida_tasks_complete_many" => {
            let pairs = arg(v, "pairs")?;
            to_json(cmd::dida::dida_tasks_complete_many(state, pairs).await)
        }
        "dida_tasks_move" => {
            let (pairs, to_project_id) = (arg(v, "pairs")?, arg(v, "to_project_id")?);
            to_json(cmd::dida::dida_tasks_move(state, pairs, to_project_id).await)
        }
        "dida_task_assign" => {
            let (project_id, task_id, username) =
                (arg(v, "project_id")?, arg(v, "task_id")?, arg(v, "username")?);
            to_json(cmd::dida::dida_task_assign(state, project_id, task_id, username).await)
        }
        "dida_task_unassign" => {
            let (project_id, task_id) = (arg(v, "project_id")?, arg(v, "task_id")?);
            to_json(cmd::dida::dida_task_unassign(state, project_id, task_id).await)
        }
        "dida_project_members" => {
            let project_id = arg(v, "project_id")?;
            to_json(cmd::dida::dida_project_members(state, project_id).await)
        }

        // ------------------------------------------------------------ catalog
        "catalog_status" => to_json(cmd::catalog::catalog_status(state).await),
        "catalog_refresh" => {
            let request = arg(v, "request")?;
            to_json(cmd::catalog::catalog_refresh(state, request).await)
        }
        "catalog_library" => {
            let query = arg(v, "query")?;
            to_json(cmd::catalog::catalog_library(state, query).await)
        }
        "catalog_releases" => {
            let query = arg(v, "query")?;
            to_json(cmd::catalog::catalog_releases(state, query).await)
        }
        "catalog_detail" => {
            let key = arg(v, "key")?;
            to_json(cmd::catalog::catalog_detail(state, key).await)
        }
        "catalog_overview" => to_json(cmd::catalog::catalog_overview(state).await),
        "catalog_jobs" => to_json(cmd::catalog::catalog_jobs(state).await),
        "catalog_source_set" => {
            let (source, enabled, token, clear_key) = (arg(v, "source")?, arg(v, "enabled")?, arg(v, "token")?, arg(v, "clear_key")?);
            to_json(cmd::catalog::catalog_source_set(state, source, enabled, token, clear_key).await)
        }
        "catalog_retry" => {
            let id = arg(v, "id")?;
            to_json(cmd::catalog::catalog_retry(state, id).await)
        }
        "catalog_facets" => {
            let query = arg(v, "query")?;
            to_json(cmd::catalog::catalog_facets(state, query).await)
        }

        // ------------------------------------------------------------ schedule
        "schedule_list" => to_json(cmd::schedule::schedule_list(state).await),
        "schedule_save" => {
            let input = arg(v, "input")?;
            to_json(cmd::schedule::schedule_save(state, input).await)
        }
        "schedule_delete" => {
            let id = arg(v, "id")?;
            to_json(cmd::schedule::schedule_delete(state, id).await)
        }
        "schedule_run" => {
            let id = arg(v, "id")?;
            to_json(cmd::schedule::schedule_run(state, id).await)
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
        "log_context" => {
            let (anchor, ts, before, after) = (
                arg(v, "anchor")?,
                arg(v, "ts")?,
                arg(v, "before")?,
                arg(v, "after")?,
            );
            to_json(cmd::logs::log_context(anchor, ts, before, after).await)
        }

        // ------------------------------------------------------------ system
        "system_health" => to_json(cmd::system::system_health(state).await),
        "diagnose_bundle" => to_json(cmd::system::diagnose_bundle(state).await),
        "platform_capabilities" => to_json(Ok(cmd::system::platform_capabilities())),
        "platform_features" => to_json(Ok(cmd::system::platform_features())),
        "update_check" => to_json(cmd::system::update_check().await),
        "config_snapshot_list" => to_json(cmd::system::config_snapshot_list(state).await),
        "config_snapshot_restore" => {
            let id = arg(v, "id")?;
            to_json(cmd::system::config_snapshot_restore(state, id).await)
        }
        "config_snapshot_clear" => to_json(cmd::system::config_snapshot_clear(state).await),

        // ------------------------------------------------------------ usage
        "usage_scan" => to_json(cmd::usage::usage_scan(state).await),
        "usage_overview" => {
            let (from, to) = (arg(v, "from")?, arg(v, "to")?);
            to_json(cmd::usage::usage_overview(state, from, to).await)
        }

        // ------------------------------------------------------------ maintenance
        "maintenance_space" => to_json(cmd::maintenance::maintenance_space().await),
        "maintenance_trash" => to_json(cmd::maintenance::maintenance_trash().await),
        "maintenance_purge_trash" => {
            let tag = arg(v, "tag")?;
            to_json(cmd::maintenance::maintenance_purge_trash(tag).await)
        }
        "maintenance_open_data" => to_json(cmd::maintenance::maintenance_open_data()),

        _ => Err(args_error(&format!("未知命令：{name}"))),
    }
}
