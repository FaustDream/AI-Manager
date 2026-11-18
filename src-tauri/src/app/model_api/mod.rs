//! 豆包 OpenAI 兼容接口：内嵌 axum，把豆包账号暴露成 `/v1/models` + `/v1/chat/completions`。
//!
//! - 只服务豆包账号（`platform == "doubao"`），其余平台不参与转发。
//! - **多账号**：所有「启用 + 有凭据」的豆包账号都参与，请求按轮询起点分散；
//!   某个账号目录/对话失败会顺延到下一个（只在吐出第一个字节前才可能换号）。
//!   模型列表取所有账号的并集。
//! - 端口默认 43861，可在设置页改（存 kv，改完立即重启监听）；只绑 `127.0.0.1`。
//!   配置端口被占用时退到系统分配的空闲端口，实际端口见 `ModelApiInfo.port`。
//! - API Key 自建（`sk-am-` 前缀），首次启动自动生成，存本地 kv；鉴权比较整段 key。
//! - 支持三种接口形态，全部翻译到豆包 `/chat/completion`：
//!   `POST /v1/chat/completions`（OpenAI Chat）、`POST /v1/messages`（Anthropic Messages）、
//!   `POST /v1/responses`（OpenAI Responses）。流式与一次性都支持。

pub mod doubao;
pub mod sse;

use crate::app::accounts::credential::Credential;
use crate::app::accounts::repo;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_PORT: u16 = 43861;
/// 配置端口被占用时顺延探测的相邻端口数（43861 占 → 试 43862…43870）。
const PORT_PROBE_SPAN: u16 = 9;
const KV_PORT: &str = "model_api.port";
const KV_KEY: &str = "model_api.key";
/// 用户是否开着这个接口。手动「停止」要能跨重启生效，所以落 kv 而不是只改内存。
const KV_ENABLED: &str = "model_api.enabled";
const KEY_PREFIX: &str = "sk-am-";
const CLIENT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/147.0.7727.149 Safari/537.36";
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// 给界面/命令行展示的接口信息。
///
/// `port` 是**实际监听**的端口；`configured_port` 是配置里的端口。两者不同
/// 说明配置端口当时被占用，服务退到了系统分配的空闲端口。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ModelApiInfo {
    pub running: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub port: u16,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub configured_port: u16,
    pub key: String,
}

/// 一个可用的豆包模型（给界面「获取模型」用）。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ModelEntry {
    /// 填进 `model` 字段的 ID（单模式是 `model_item_key`，多模式是 `mode::model`）
    pub id: String,
    /// 豆包侧展示名
    pub name: String,
}

/// 一次接口调用的摘要（界面「最近调用」列表）。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct CallRecord {
    pub ts: String,
    /// 请求路径（`/v1/chat/completions` …）
    pub path: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub status: u16,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ms: u64,
    pub ok: bool,
}

/// 接口运行统计（界面只读展示；重启后从零开始）。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ModelApiStats {
    pub running: bool,
    /// 本次监听启动时间；从未启动过为 None
    pub started_at: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub uptime_secs: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub requests: u64,
    /// 非 2xx 的调用数（含鉴权失败与上游错误）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub failures: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub avg_ms: u64,
    pub last_request_at: Option<String>,
    /// 最近 20 次调用，时间倒序
    pub recent: Vec<CallRecord>,
}

/// 自检结果：带 Key 真发一次 `/v1/models`，验证「监听 + 鉴权 + 上游」整条链路。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ModelApiProbe {
    pub ok: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub status: u16,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ms: u64,
    pub url: String,
    pub detail: String,
}

/// 进程内的调用计数与最近记录。
struct Stats {
    started_at: std::sync::OnceLock<String>,
    started: std::sync::OnceLock<Instant>,
    requests: AtomicU64,
    failures: AtomicU64,
    total_ms: AtomicU64,
    last_at: Mutex<Option<String>>,
    recent: Mutex<VecDeque<CallRecord>>,
}

impl Stats {
    fn new() -> Self {
        Self {
            started_at: std::sync::OnceLock::new(),
            started: std::sync::OnceLock::new(),
            requests: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            total_ms: AtomicU64::new(0),
            last_at: Mutex::new(None),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    fn mark_started(&self) {
        let _ = self.started_at.set(chrono::Local::now().to_rfc3339());
        let _ = self.started.set(Instant::now());
    }

    fn record(&self, path: &str, status: u16, ms: u64, ok: bool) {
        let ts = chrono::Local::now().to_rfc3339();
        if let Ok(mut last) = self.last_at.lock() {
            *last = Some(ts.clone());
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.total_ms.fetch_add(ms, Ordering::Relaxed);
        if !ok {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut recent) = self.recent.lock() {
            if recent.len() >= 20 {
                recent.pop_back();
            }
            recent.push_front(CallRecord {
                ts,
                path: path.to_string(),
                status,
                ms,
                ok,
            });
        }
    }

    fn snapshot(&self, running: bool) -> ModelApiStats {
        let requests = self.requests.load(Ordering::Relaxed);
        let total_ms = self.total_ms.load(Ordering::Relaxed);
        ModelApiStats {
            running,
            started_at: self.started_at.get().cloned(),
            // 停掉之后不能再按 Instant 继续累加：那会把「停止后又过了几小时」画成运行时长
            uptime_secs: if running {
                self.started
                    .get()
                    .map(|at| at.elapsed().as_secs())
                    .unwrap_or(0)
            } else {
                0
            },
            requests,
            failures: self.failures.load(Ordering::Relaxed),
            avg_ms: total_ms.checked_div(requests).unwrap_or(0),
            last_request_at: self.last_at.lock().ok().and_then(|v| v.clone()),
            recent: self
                .recent
                .lock()
                .map(|v| v.iter().cloned().collect())
                .unwrap_or_default(),
        }
    }
}

/// 一个豆包账号的转发会话。
#[derive(Clone)]
struct Session {
    cookie: String,
    seed: String,
    fp: String,
}

/// 参与转发的账号：会话 + 用于日志/报错的展示名。
#[derive(Clone)]
struct Target {
    iid: String,
    label: String,
    session: Session,
}

/// 带缓存的模型目录。
#[derive(Clone)]
struct Catalog {
    bot: String,
    models: BTreeMap<String, doubao::ModelInfo>,
    at: i64,
}

/// 路由共享状态。
#[derive(Clone)]
struct ApiState {
    db: Arc<Db>,
    client: reqwest::Client,
    key: Arc<RwLock<String>>,
    cache: Arc<Mutex<BTreeMap<String, Catalog>>>,
    /// 轮询起点：每次请求 +1，让多个豆包账号轮流承载。
    rr: Arc<AtomicUsize>,
    stats: Arc<Stats>,
}

/// 服务器单例：同一时间只有一个监听。
struct Server {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    /// 实际监听端口（端口被占用时可能 != configured）。
    port: u16,
    /// 配置里的端口。
    configured: u16,
    key: Arc<RwLock<String>>,
    stats: Arc<Stats>,
}

fn server() -> &'static Mutex<Server> {
    static S: OnceLock<Mutex<Server>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Server {
            shutdown: None,
            port: DEFAULT_PORT,
            configured: DEFAULT_PORT,
            key: Arc::new(RwLock::new(String::new())),
            stats: Arc::new(Stats::new()),
        })
    })
}

// ------------------------------------------------------------------ 生命周期

/// 启动模型接口（用户点「启动」时调用）。已启动则直接返回现状。
pub async fn start(db: Arc<Db>) -> Result<ModelApiInfo> {
    set_enabled(&db, true).await?;
    ensure_started(db).await
}

/// 应用启动时调用：用户上次手动「停止」过就不再自动起，避免自作主张把服务拉回来。
pub async fn autostart(db: Arc<Db>) -> Result<Option<ModelApiInfo>> {
    let enabled = crate::app::runtime::blocking({
        let db = db.clone();
        move || enabled_pref(&db)
    })
    .await?;
    if !enabled {
        return Ok(None);
    }
    start(db).await.map(Some)
}

/// 真正把监听拉起来（不改「用户是否启用」的偏好）。
async fn ensure_started(db: Arc<Db>) -> Result<ModelApiInfo> {
    let (port, key) = crate::app::runtime::blocking({
        let db = db.clone();
        move || load_or_init(&db)
    })
    .await??;

    // 已启动就直接回现状。锁只在同步块里短暂持有，不跨 await（否则 future 不 Send）
    {
        let srv = server()
            .lock()
            .map_err(|_| AppError::new(ErrorKind::Io, "模型接口状态锁失效"))?;
        if srv.shutdown.is_some() {
            return Ok(snapshot(&srv));
        }
    }

    serve(db, port, key).await
}

/// 用户点「停止」：先落偏好再停监听，重启后也不会自己回来。
pub async fn stop(db: Arc<Db>) -> Result<ModelApiInfo> {
    set_enabled(&db, false).await?;
    stop_listening();
    // 等监听真正释放，接口状态别在响应里还报「运行中」
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut srv = server()
        .lock()
        .map_err(|_| AppError::new(ErrorKind::Io, "模型接口状态锁失效"))?;
    srv.shutdown = None;
    Ok(snapshot(&srv))
}

/// 改端口并立即生效：写 kv → 停旧监听 → 用新端口重启。
/// 没在跑就只写配置（下次启动生效）。
pub async fn set_port(db: Arc<Db>, port: u16) -> Result<ModelApiInfo> {
    if port < 1024 {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "端口需在 1024–65535 之间",
        ));
    }
    crate::app::runtime::blocking({
        let db = db.clone();
        move || db.kv_set(KV_PORT, &port.to_string())
    })
    .await??;

    // 没在跑：只落配置。别顺手把服务拉起来——启动时机由应用启动流程控制。
    let running = server()
        .lock()
        .map(|srv| srv.shutdown.is_some())
        .unwrap_or(false);
    if !running {
        let mut srv = server()
            .lock()
            .map_err(|_| AppError::new(ErrorKind::Io, "模型接口状态锁失效"))?;
        srv.configured = port;
        return Ok(snapshot(&srv));
    }

    // 停旧监听，给内核一点时间释放端口，再用新端口起
    stop_listening();
    tokio::time::sleep(Duration::from_millis(250)).await;

    let key = crate::app::runtime::blocking({
        let db = db.clone();
        move || load_key(&db)
    })
    .await??;
    serve(db, port, key).await
}

/// 重启监听（换端口、改 Key 之外的排障手段）：停旧监听 → 用配置端口重新绑定。
/// 没在跑时等价于启动，并把「用户要它开着」的偏好写回。
pub async fn restart(db: Arc<Db>) -> Result<ModelApiInfo> {
    set_enabled(&db, true).await?;
    stop_listening();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let port = crate::app::runtime::blocking({
        let db = db.clone();
        move || configured_port(&db)
    })
    .await?;
    let key = crate::app::runtime::blocking({
        let db = db.clone();
        move || load_key(&db)
    })
    .await??;
    serve(db, port, key).await
}

/// 绑定监听并起服务。
///
/// 端口被占用时**不报错退出**，而是先顺延相邻端口、再退系统空闲端口——否则整个
/// 接口起不来，用户只能靠日志猜原因。实际端口体现在 `ModelApiInfo.port` 上。
async fn serve(db: Arc<Db>, want: u16, key: String) -> Result<ModelApiInfo> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent("AI-Manager")
        .build()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("构建 HTTP 客户端失败：{e}")))?;

    // key 用 Arc 共享：regenerate_key 改的是同一个 RwLock，正在跑的接口立即生效
    let key_arc = Arc::new(RwLock::new(key));
    let stats = Arc::new(Stats::new());
    stats.mark_started();
    let state = ApiState {
        db,
        client,
        key: key_arc.clone(),
        cache: Arc::new(Mutex::new(BTreeMap::new())),
        rr: Arc::new(AtomicUsize::new(0)),
        stats: stats.clone(),
    };
    let app = router(state);

    let (listener, port) = bind(want).await?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    tauri::async_runtime::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });

    let mut srv = server()
        .lock()
        .map_err(|_| AppError::new(ErrorKind::Io, "模型接口状态锁失效"))?;
    srv.shutdown = Some(tx);
    srv.port = port;
    srv.configured = want;
    srv.key = key_arc;
    srv.stats = stats;
    Ok(snapshot(&srv))
}

/// 先试配置端口；占用就顺延试相邻端口（端口号相对稳定，客户端地址不用每次改）；
/// 都不行才退到系统分配的空闲端口。返回监听器与实际端口。
async fn bind(want: u16) -> Result<(tokio::net::TcpListener, u16)> {
    for offset in 0..=PORT_PROBE_SPAN {
        let Some(candidate) = want.checked_add(offset).filter(|p| *p >= 1024) else {
            break;
        };
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", candidate)).await {
            if offset > 0 {
                crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "model_api",
                    format!("配置端口 {want} 被占用，已改用相邻端口 {candidate}"),
                );
            }
            return Ok((listener, candidate));
        }
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, format!("模型接口无法监听本机端口：{e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("读取监听端口失败：{e}")))?
        .port();
    crate::app::log::log(
        crate::app::log::Level::Warn,
        "model_api",
        format!("配置端口 {want} 及相邻端口都被占用，已改用空闲端口 {port}"),
    );
    Ok((listener, port))
}

/// 只停监听，不动「用户是否启用」的偏好（换端口、测试用）。
fn stop_listening() {
    if let Ok(mut srv) = server().lock() {
        if let Some(tx) = srv.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// 用户是否允许这个接口运行（默认允许；只有明确写过 false 才不自动起）。
pub fn enabled_pref(db: &Db) -> bool {
    db.kv_get(KV_ENABLED)
        .ok()
        .flatten()
        .map(|v| v.trim() != "false")
        .unwrap_or(true)
}

async fn set_enabled(db: &Arc<Db>, on: bool) -> Result<()> {
    let db = db.clone();
    crate::app::runtime::blocking(move || db.kv_set(KV_ENABLED, if on { "true" } else { "false" }))
        .await?
}

/// 重新生成 API Key 并立即生效（写 kv + 更新内存里的 key，无需重启）。
pub async fn regenerate_key(db: Arc<Db>) -> Result<ModelApiInfo> {
    let key = fresh_key();
    crate::app::runtime::blocking({
        let db = db.clone();
        let key = key.clone();
        move || db.kv_set(KV_KEY, &key)
    })
    .await??;

    let srv = server()
        .lock()
        .map_err(|_| AppError::new(ErrorKind::Io, "模型接口状态锁失效"))?;
    *srv.key.write().unwrap_or_else(|e| e.into_inner()) = key;
    Ok(snapshot(&srv))
}

/// 取当前可用模型列表（界面「获取模型」用）。
///
/// 不经过本地接口、也不需要 API Key，直接用本地豆包账号的凭据拉一次目录——
/// 这样接口停着也能查模型，用户不必先跑起来再看。
/// 返回**所有账号的并集**：任一账号有就能用，客户端不必关系具体落哪个号。
pub async fn available_models(db: Arc<Db>) -> Result<Vec<ModelEntry>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("AI-Manager")
        .build()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("构建 HTTP 客户端失败：{e}")))?;
    let state = ApiState {
        db,
        client,
        key: Arc::new(RwLock::new(String::new())),
        cache: Arc::new(Mutex::new(BTreeMap::new())),
        rr: Arc::new(AtomicUsize::new(0)),
        stats: Arc::new(Stats::new()),
    };
    let targets = load_targets(&state.db).await?;
    if targets.is_empty() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "没有可用的豆包账号，请先在账号中心添加并启用",
        ));
    }
    let mut merged: BTreeMap<String, ModelEntry> = BTreeMap::new();
    let mut last_err = String::new();
    let mut ok = 0usize;
    for t in &targets {
        match resolve_catalog(&state, t).await {
            Ok(catalog) => {
                ok += 1;
                for (id, m) in catalog.models {
                    let name = if m.name.is_empty() {
                        id.clone()
                    } else {
                        m.name
                    };
                    merged.entry(id.clone()).or_insert(ModelEntry { id, name });
                }
            }
            Err(e) => last_err = format!("{}：{}", t.label, e.message),
        }
    }
    if ok == 0 {
        return Err(AppError::new(
            ErrorKind::Provider,
            format!("拉取豆包模型目录失败（{last_err}）"),
        ));
    }
    Ok(merged.into_values().collect())
}

pub fn info() -> ModelApiInfo {
    server()
        .lock()
        .map(|srv| snapshot(&srv))
        .unwrap_or_else(|_| ModelApiInfo {
            running: false,
            port: DEFAULT_PORT,
            configured_port: DEFAULT_PORT,
            key: String::new(),
        })
}

/// 当前运行统计（界面「运行概览」；停止后仍保留上一次监听的数字）。
pub fn stats() -> ModelApiStats {
    match server().lock() {
        Ok(srv) => srv.stats.snapshot(srv.shutdown.is_some()),
        Err(_) => ModelApiStats {
            running: false,
            started_at: None,
            uptime_secs: 0,
            requests: 0,
            failures: 0,
            avg_ms: 0,
            last_request_at: None,
            recent: Vec::new(),
        },
    }
}

/// 带 Key 真发一次 `/v1/models`：验证「监听 → 鉴权 → 上游目录」整条链路。
/// 失败也返回结果（`status` 为 0 表示连接层失败），界面按 `ok` 决定颜色。
pub async fn probe() -> Result<ModelApiProbe> {
    let (running, port, key) = {
        let srv = server()
            .lock()
            .map_err(|_| AppError::new(ErrorKind::Io, "模型接口状态锁失效"))?;
        // guard 必须先落到局部变量再进元组：临时值的 Drop 会早于 srv，借用检查过不去
        let key = srv.key.read().unwrap_or_else(|e| e.into_inner()).clone();
        (srv.shutdown.is_some(), srv.port, key)
    };
    if !running {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "接口未启动，请先点「启动」再自检",
        ));
    }
    let url = format!("http://127.0.0.1:{port}/v1/models");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .no_proxy()
        .user_agent("AI-Manager")
        .build()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("构建 HTTP 客户端失败：{e}")))?;
    let started = Instant::now();
    let result = client.get(&url).bearer_auth(&key).send().await;
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let ok = resp.status().is_success();
            let body = resp.text().await.unwrap_or_default();
            let detail = if ok {
                "监听、鉴权与上游模型目录均正常".to_string()
            } else if status == 401 {
                "API Key 被拒绝：接口里的 Key 与界面不一致，重新生成后再试".to_string()
            } else {
                let brief: String = body.chars().take(200).collect();
                format!("HTTP {status}：{brief}")
            };
            Ok(ModelApiProbe {
                ok,
                status,
                ms,
                url,
                detail,
            })
        }
        Err(e) => Ok(ModelApiProbe {
            ok: false,
            status: 0,
            ms,
            url,
            detail: format!(
                "连接失败：{}",
                if e.is_timeout() {
                    "请求超时"
                } else if e.is_connect() {
                    "连不上本机监听"
                } else {
                    "请求失败"
                }
            ),
        }),
    }
}

fn snapshot(srv: &Server) -> ModelApiInfo {
    ModelApiInfo {
        running: srv.shutdown.is_some(),
        port: srv.port,
        configured_port: srv.configured,
        key: srv.key.read().unwrap_or_else(|e| e.into_inner()).clone(),
    }
}

fn load_or_init(db: &Db) -> Result<(u16, String)> {
    Ok((configured_port(db), load_key(db)?))
}

/// 配置里的端口（非法/未设置时回默认值）。界面以它为准，避免内存单例与实际配置打架。
pub fn configured_port(db: &Db) -> u16 {
    db.kv_get(KV_PORT)
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p >= 1024)
        .unwrap_or(DEFAULT_PORT)
}

/// 读 API Key；没有就生成一个写回。
fn load_key(db: &Db) -> Result<String> {
    match db.kv_get(KV_KEY)? {
        Some(k) if !k.is_empty() => Ok(k),
        _ => {
            let k = fresh_key();
            db.kv_set(KV_KEY, &k)?;
            Ok(k)
        }
    }
}

fn fresh_key() -> String {
    let mut buf = [0u8; 32];
    let _ = getrandom::getrandom(&mut buf);
    let mut text = String::with_capacity(KEY_PREFIX.len() + buf.len() * 2);
    text.push_str(KEY_PREFIX);
    for b in buf {
        text.push_str(&format!("{b:02x}"));
    }
    text
}

// ------------------------------------------------------------------ 路由

fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        // Anthropic 形态：Claude 系客户端（Claude Code、各类 Anthropic SDK）走这里
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/messages/count_tokens", post(anthropic_count_tokens))
        // OpenAI Responses 形态：新版 SDK/客户端用 `/responses`
        .route("/v1/responses", post(responses_create))
        .route("/responses", post(responses_create))
        // 统一在路由层记调用：每个 handler 自己记会漏掉鉴权失败与 404，
        // 而那正是「客户端连不上」时最需要看到的证据。
        .layer(middleware::from_fn_with_state(state.clone(), track_call))
        .with_state(state)
}

/// 记录每次调用的路径、状态码与耗时（不含请求体，避免把对话内容落进统计）。
async fn track_call(State(state): State<ApiState>, req: Request, next: Next) -> Response {
    let started = Instant::now();
    let path = req.uri().path().to_string();
    let resp = next.run(req).await;
    let status = resp.status().as_u16();
    state.stats.record(
        &path,
        status,
        started.elapsed().as_millis() as u64,
        resp.status().is_success(),
    );
    resp
}

async fn list_models(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !authorize(&headers, &key_of(&state)) {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "API Key 无效或已停用",
        );
    }
    let targets = match load_targets(&state.db).await {
        Ok(t) => t,
        Err(e) => return api_error(StatusCode::BAD_GATEWAY, "upstream_error", &e.message),
    };
    if targets.is_empty() {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_available_account",
            "没有可用的豆包账号，请先在账号中心添加并启用",
        );
    }
    // 所有账号的并集：客户端只认一份列表，不该因为某个账号少一个模型就看不见
    let mut ids = BTreeSet::new();
    let mut last_err = String::new();
    let mut ok = 0usize;
    for t in &targets {
        match resolve_catalog(&state, t).await {
            Ok(c) => {
                ok += 1;
                ids.extend(c.models.into_keys());
            }
            Err(e) => last_err = format!("{}：{}", t.label, e.message),
        }
    }
    if ok == 0 {
        return api_error(
            StatusCode::BAD_GATEWAY,
            "upstream_error",
            &format!("拉取豆包模型目录失败（{last_err}）"),
        );
    }
    let data: Vec<Value> = ids
        .into_iter()
        .map(|id| json!({ "id": id, "object": "model", "owned_by": "doubao" }))
        .collect();
    Json(json!({ "object": "list", "data": data })).into_response()
}

async fn chat_completions(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorize(&headers, &key_of(&state)) {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "API Key 无效或已停用",
        );
    }
    let req = match parse_chat(&body) {
        Ok(req) => req,
        Err(msg) => return api_error(StatusCode::BAD_REQUEST, "invalid_request_error", &msg),
    };
    let id = format!("chatcmpl-{}", doubao::uuid());
    let created = crate::app::timeutil::now_ms() / 1000;
    match open_chat(&state, &req.model, &req.messages).await {
        Ok(resp) => {
            if req.stream {
                chat_stream(spawn_deltas(resp), &req.model, id, created)
            } else {
                match collect_text(spawn_deltas(resp)).await {
                    Ok((content, reasoning)) if !(content.is_empty() && reasoning.is_empty()) => {
                        chat_completion(&id, created, &req.model, &content, &reasoning)
                    }
                    Ok(_) => api_error(
                        StatusCode::BAD_GATEWAY,
                        "upstream_empty_response",
                        "豆包未返回文本",
                    ),
                    Err(e) => api_error(StatusCode::BAD_GATEWAY, "upstream_error", &e),
                }
            }
        }
        Err(e) => {
            let (status, kind, message) = e.parts();
            api_error(status, kind, &message)
        }
    }
}

/// Anthropic Messages（`POST /v1/messages`）：Claude 系客户端的标准入口。
///
/// 请求/响应都按 Anthropic 的形状翻译，思维链不外露（Anthropic 侧要显式开 thinking
/// 才会收到 thinking 块，我们没实现这个开关，所以只给正文）。
async fn anthropic_messages(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorize(&headers, &key_of(&state)) {
        return anthropic_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "API Key 无效或已停用",
        );
    }
    let req = match parse_anthropic(&body) {
        Ok(req) => req,
        Err(msg) => return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &msg),
    };
    let id = format!("msg_{}", doubao::uuid());
    match open_chat(&state, &req.model, &req.messages).await {
        Ok(resp) => {
            if req.stream {
                anthropic_stream(spawn_deltas(resp), &req.model, id)
            } else {
                match collect_text(spawn_deltas(resp)).await {
                    Ok((content, _)) if !content.trim().is_empty() => {
                        Json(anthropic_message(&id, &req.model, &content)).into_response()
                    }
                    Ok(_) => {
                        anthropic_error(StatusCode::BAD_GATEWAY, "api_error", "豆包未返回文本")
                    }
                    Err(e) => anthropic_error(StatusCode::BAD_GATEWAY, "api_error", &e),
                }
            }
        }
        Err(e) => {
            let (status, _, message) = e.parts();
            anthropic_error(status, anthropic_kind(status), &message)
        }
    }
}

/// Anthropic 的 token 预估。客户端拿它做上下文裁剪，回一个「够用就行」的估算值。
async fn anthropic_count_tokens(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorize(&headers, &key_of(&state)) {
        return anthropic_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "API Key 无效或已停用",
        );
    }
    let req = match parse_anthropic(&body) {
        Ok(req) => req,
        Err(msg) => return anthropic_error(StatusCode::BAD_REQUEST, "invalid_request_error", &msg),
    };
    let chars: usize = req.messages.iter().map(|m| m.content.chars().count()).sum();
    Json(json!({ "input_tokens": est_tokens(chars) })).into_response()
}

/// OpenAI Responses（`POST /v1/responses`）：新版 SDK 的默认入口，事件流也按它翻译。
async fn responses_create(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorize(&headers, &key_of(&state)) {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "API Key 无效或已停用",
        );
    }
    let req = match parse_responses(&body) {
        Ok(req) => req,
        Err(msg) => return api_error(StatusCode::BAD_REQUEST, "invalid_request_error", &msg),
    };
    let id = format!("resp_{}", doubao::uuid());
    let item_id = format!("msg_{}", doubao::uuid());
    let created = crate::app::timeutil::now_ms() / 1000;
    match open_chat(&state, &req.model, &req.messages).await {
        Ok(resp) => {
            if req.stream {
                responses_stream(spawn_deltas(resp), &req.model, &id, &item_id, created)
            } else {
                match collect_text(spawn_deltas(resp)).await {
                    Ok((content, _)) if !content.trim().is_empty() => Json(response_object(
                        &id,
                        &item_id,
                        &req.model,
                        created,
                        "completed",
                        &content,
                    ))
                    .into_response(),
                    Ok(_) => api_error(
                        StatusCode::BAD_GATEWAY,
                        "upstream_empty_response",
                        "豆包未返回文本",
                    ),
                    Err(e) => api_error(StatusCode::BAD_GATEWAY, "upstream_error", &e),
                }
            }
        }
        Err(e) => {
            let (status, kind, message) = e.parts();
            api_error(status, kind, &message)
        }
    }
}

// ------------------------------------------------------------------ 请求处理

struct ChatRequest {
    model: String,
    messages: Vec<doubao::ChatMessage>,
    stream: bool,
}

fn parse_chat(body: &Value) -> std::result::Result<ChatRequest, String> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 160)
        .ok_or_else(|| "请提供模型 ID".to_string())?;
    let stream = match body.get("stream") {
        Some(v) => v.as_bool().ok_or_else(|| "stream 应为布尔值".to_string())?,
        None => false,
    };
    let response_type_ok = body
        .get("response_format")
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        .map(|t| t == "text")
        .unwrap_or(true);
    if body.get("tools").is_some()
        || body.get("functions").is_some()
        || body.get("tool_choice").is_some()
        || !response_type_ok
        || body
            .get("n")
            .and_then(Value::as_i64)
            .map(|n| n != 1)
            .unwrap_or(false)
    {
        return Err("此版本仅支持单路纯文本，工具调用与结构化输出尚未接入".to_string());
    }
    let arr = body
        .get("messages")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= 100)
        .ok_or_else(|| "请提供 1–100 条消息".to_string())?;
    let mut messages = Vec::with_capacity(arr.len());
    for m in arr {
        let role = m
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "消息缺少角色".to_string())?;
        if !matches!(role, "system" | "developer" | "user" | "assistant")
            || m.get("tool_calls").is_some()
        {
            return Err("此版本仅支持文本对话角色".to_string());
        }
        let content = match m.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts))
                if parts.iter().all(|p| {
                    p.get("type").and_then(Value::as_str) == Some("text")
                        && p.get("text").and_then(Value::as_str).is_some()
                }) =>
            {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => return Err("消息正文须为文本".to_string()),
        };
        messages.push(doubao::ChatMessage {
            role: role.to_string(),
            content,
        });
    }
    if !messages
        .iter()
        .any(|m| m.role == "user" && !m.content.trim().is_empty())
    {
        return Err("至少需要一条非空用户消息".to_string());
    }
    Ok(ChatRequest {
        model: model.to_string(),
        messages,
        stream,
    })
}

// ------------------------------------------------------------ 各形态的请求解析

/// 模型 ID：三种形态都叫 `model`。
fn model_of(body: &Value) -> std::result::Result<&str, String> {
    body.get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 160)
        .ok_or_else(|| "请提供模型 ID".to_string())
}

/// `stream` 开关（缺省 false）。
fn stream_flag(body: &Value) -> std::result::Result<bool, String> {
    match body.get("stream") {
        Some(v) => v.as_bool().ok_or_else(|| "stream 应为布尔值".to_string()),
        None => Ok(false),
    }
}

/// 把「文本内容」折成字符串：既接受纯字符串，也接受内容块数组。
///
/// 三种形态的块名不一样（`text` / `input_text` / `output_text`），但语义都是正文；
/// 遇到图片、工具调用这类**没接入**的块时返回 None，由调用方给出明确报错，
/// 而不是悄悄丢掉内容。
fn text_blocks(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let mut out = Vec::with_capacity(parts.len());
            for p in parts {
                let ty = p.get("type").and_then(Value::as_str).unwrap_or("text");
                if !matches!(ty, "text" | "input_text" | "output_text") {
                    return None;
                }
                out.push(p.get("text").and_then(Value::as_str)?.to_string());
            }
            Some(out.join("\n"))
        }
        _ => None,
    }
}

/// 至少要有一条非空用户消息，否则豆包那边没话可说。
fn ensure_user(messages: &[doubao::ChatMessage]) -> std::result::Result<(), String> {
    if messages
        .iter()
        .any(|m| m.role == "user" && !m.content.trim().is_empty())
    {
        Ok(())
    } else {
        Err("至少需要一条非空用户消息".to_string())
    }
}

/// Anthropic Messages 请求 → 内部消息序列。
fn parse_anthropic(body: &Value) -> std::result::Result<ChatRequest, String> {
    if body.get("tools").is_some() || body.get("tool_choice").is_some() {
        return Err("此版本仅支持纯文本对话，工具调用尚未接入".to_string());
    }
    let model = model_of(body)?;
    let stream = stream_flag(body)?;
    let mut messages: Vec<doubao::ChatMessage> = Vec::new();
    if let Some(sys) = body.get("system") {
        let text = text_blocks(sys).ok_or_else(|| "system 须为文本".to_string())?;
        if !text.trim().is_empty() {
            messages.push(doubao::ChatMessage {
                role: "system".into(),
                content: text,
            });
        }
    }
    let arr = body
        .get("messages")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= 100)
        .ok_or_else(|| "请提供 1–100 条消息".to_string())?;
    for m in arr {
        let role = m
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "消息缺少角色".to_string())?;
        if !matches!(role, "user" | "assistant") {
            return Err("Anthropic messages 只允许 user / assistant 角色".to_string());
        }
        let raw = m.get("content").ok_or_else(|| "消息缺少正文".to_string())?;
        let content = text_blocks(raw).ok_or_else(|| "消息正文须为文本".to_string())?;
        messages.push(doubao::ChatMessage {
            role: role.to_string(),
            content,
        });
    }
    ensure_user(&messages)?;
    Ok(ChatRequest {
        model: model.to_string(),
        messages,
        stream,
    })
}

/// OpenAI Responses 请求 → 内部消息序列。
///
/// `input` 既可能是整段字符串，也可能是消息数组；`instructions` 等价于 system。
fn parse_responses(body: &Value) -> std::result::Result<ChatRequest, String> {
    if body.get("tools").is_some() || body.get("tool_choice").is_some() {
        return Err("此版本仅支持纯文本对话，工具调用尚未接入".to_string());
    }
    let model = model_of(body)?;
    let stream = stream_flag(body)?;
    let mut messages: Vec<doubao::ChatMessage> = Vec::new();
    if let Some(ins) = body.get("instructions") {
        if let Some(text) = text_blocks(ins) {
            if !text.trim().is_empty() {
                messages.push(doubao::ChatMessage {
                    role: "system".into(),
                    content: text,
                });
            }
        }
    }
    match body.get("input") {
        Some(Value::String(s)) if !s.trim().is_empty() => messages.push(doubao::ChatMessage {
            role: "user".into(),
            content: s.clone(),
        }),
        Some(Value::Array(items)) if !items.is_empty() && items.len() <= 100 => {
            for it in items {
                let role = it.get("role").and_then(Value::as_str).unwrap_or("user");
                if !matches!(role, "system" | "developer" | "user" | "assistant") {
                    return Err("input 里的角色不受支持".to_string());
                }
                let raw = it
                    .get("content")
                    .ok_or_else(|| "input 条目缺少 content".to_string())?;
                let content = text_blocks(raw).ok_or_else(|| "input 内容须为文本".to_string())?;
                messages.push(doubao::ChatMessage {
                    role: role.to_string(),
                    content,
                });
            }
        }
        _ => return Err("请提供 input（字符串或消息数组）".to_string()),
    }
    ensure_user(&messages)?;
    Ok(ChatRequest {
        model: model.to_string(),
        messages,
        stream,
    })
}

/// token 粗估：中文约 1 字 1 token、英文约 4 字符 1 token，取中间当量。
/// 只用于 usage 展示与上下文裁剪，不追求准确。
fn est_tokens(chars: usize) -> usize {
    chars.div_ceil(3).max(1)
}

/// 取出**所有**启用且带凭据的豆包账号，按账号中心的排序返回。
///
/// 全部参与转发：请求按轮询起点分散到各账号，某个账号不可用时顺延到下一个。
/// 单个账号凭据解析失败只跳过它（记 Warn），不能让整条链路连带失败。
async fn load_targets(db: &Arc<Db>) -> Result<Vec<Target>> {
    crate::app::runtime::blocking({
        let db = db.clone();
        move || {
            let rows = repo::list(&db)?;
            let mut out = Vec::new();
            for row in rows
                .iter()
                .filter(|r| r.platform == "doubao" && r.has_secret && r.enabled)
            {
                let Some(bytes) = repo::secret_of(&db, &row.iid)? else {
                    continue;
                };
                let cred = match Credential::parse(&bytes) {
                    Ok(c) => c,
                    Err(e) => {
                        crate::app::log::log(
                            crate::app::log::Level::Warn,
                            "model_api",
                            format!("账号 {} 凭据无法解析，跳过：{}", row.iid, e.message),
                        );
                        continue;
                    }
                };
                let token = if cred.token.is_empty() {
                    cred.meta_str("cookie").unwrap_or("").to_string()
                } else {
                    cred.token.clone()
                };
                if token.trim().is_empty() {
                    continue;
                }
                let seed = cred
                    .meta_str("seed")
                    .or_else(|| cred.meta_str("uid"))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    // 没有 seed/uid 时按账号 iid 兜底：以前统一退回常量 "doubao"，
                    // 多账号会算出同一组设备标识（web_id），等于让豆包把几个号认成一台设备。
                    .unwrap_or(row.iid.as_str())
                    .to_string();
                let (cookie, seed) = doubao::session_of(&token, &seed);
                let fp = doubao::cookie_value(&cookie, "s_v_web_id")
                    .or_else(|| doubao::cookie_value(&cookie, "fp"))
                    .unwrap_or_default();
                let label = row
                    .alias
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&row.name)
                    .to_string();
                out.push(Target {
                    iid: row.iid.clone(),
                    label,
                    session: Session { cookie, seed, fp },
                });
            }
            Ok(out)
        }
    })
    .await?
}

/// 解析某个账号的模型目录（带 5 分钟内存缓存，按账号 iid 区分）。
///
/// 目录是**按账号**的：不同账号的可选模型不一定一致，所以缓存与查询都不能跨账号复用。
async fn resolve_catalog(state: &ApiState, target: &Target) -> Result<Catalog> {
    let cache_key = format!("{}|{}", target.iid, target.session.cookie.len());
    let now = crate::app::timeutil::now_ms();
    if let Some(cached) = state
        .cache
        .lock()
        .ok()
        .and_then(|c| c.get(&cache_key).cloned())
    {
        if now - cached.at < 5 * 60 * 1000 {
            return Ok(cached);
        }
    }
    let catalog = fetch_catalog(&state.client, &target.session).await?;
    if let Ok(mut c) = state.cache.lock() {
        c.insert(cache_key, catalog.clone());
    }
    Ok(catalog)
}

async fn fetch_catalog(client: &reqwest::Client, session: &Session) -> Result<Catalog> {
    let headers = common_headers(&session.cookie);
    let launch: Value = client
        .post(doubao::endpoint(
            "/alice/user/launch",
            &session.seed,
            &session.fp,
        ))
        .headers(headers.clone())
        .json(&json!({}))
        .send()
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, format!("连不上豆包：{e}")))?
        .json()
        .await
        .map_err(|e| AppError::new(ErrorKind::Provider, format!("豆包返回异常：{e}")))?;
    let bot = doubao::parse_launch(&launch)
        .ok_or_else(|| AppError::new(ErrorKind::Provider, "豆包未返回助手信息"))?;
    let models: Value = client
        .post(doubao::endpoint(
            "/alice/slot/action_bar_v3/brief_list",
            &session.seed,
            &session.fp,
        ))
        .headers(headers)
        .json(&doubao::catalog_body(&bot))
        .send()
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, format!("连不上豆包：{e}")))?
        .json()
        .await
        .map_err(|e| AppError::new(ErrorKind::Provider, format!("豆包返回异常：{e}")))?;
    let models = doubao::parse_catalog(&models);
    if models.is_empty() {
        return Err(AppError::new(
            ErrorKind::Provider,
            "豆包模型目录为空或已变化",
        ));
    }
    Ok(Catalog {
        bot,
        models,
        at: crate::app::timeutil::now_ms(),
    })
}

fn common_headers(cookie: &str) -> header::HeaderMap {
    let mut h = header::HeaderMap::new();
    h.insert(header::COOKIE, hv(cookie));
    h.insert(header::ORIGIN, hv("https://www.doubao.com"));
    h.insert(header::REFERER, hv("https://www.doubao.com/chat/"));
    h.insert("Agw-Js-Conv", hv("str"));
    h.insert(header::USER_AGENT, hv(CLIENT_UA));
    h
}

/// 组装 header 值。这些值都是本模块控制的 ASCII，解析失败时退回空串而不是崩。
fn hv(value: &str) -> header::HeaderValue {
    value
        .parse()
        .unwrap_or_else(|_| header::HeaderValue::from_static(""))
}

/// 发一次豆包对话请求，返回 SSE 响应。
async fn send_chat(
    client: &reqwest::Client,
    session: &Session,
    payload: &Value,
) -> Result<reqwest::Response> {
    let mut headers = common_headers(&session.cookie);
    headers.insert(
        header::ACCEPT,
        "text/event-stream, application/json".parse().unwrap(),
    );
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    let resp = client
        .post(doubao::endpoint(
            "/chat/completion",
            &session.seed,
            &session.fp,
        ))
        .headers(headers)
        .body(payload.to_string())
        .send()
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, format!("连不上豆包：{e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::new(
            ErrorKind::Provider,
            format!("豆包对话失败：HTTP {}", resp.status().as_u16()),
        ));
    }
    Ok(resp)
}

// ------------------------------------------------------------------ 上游链路

/// 打开上游对话失败的原因。三种形态各自翻译成自己的错误格式。
enum OpenError {
    NoAccount,
    ModelNotFound,
    Upstream(String),
}

impl OpenError {
    /// (HTTP 状态, 错误类型, 人话消息)
    fn parts(&self) -> (StatusCode, &'static str, String) {
        match self {
            OpenError::NoAccount => (
                StatusCode::SERVICE_UNAVAILABLE,
                "no_available_account",
                "没有可用的豆包账号，请先在账号中心添加并启用".to_string(),
            ),
            OpenError::ModelNotFound => (
                StatusCode::NOT_FOUND,
                "model_not_found",
                "模型未接入或不在可用范围内".to_string(),
            ),
            OpenError::Upstream(m) => (StatusCode::BAD_GATEWAY, "upstream_error", m.clone()),
        }
    }
}

/// 按轮询顺序挑账号，直到某个账号接受这次请求，返回**已建立**的上游响应。
///
/// 先建连再返回，是为了让调用方能安全地「失败换号」：一旦开始向客户端推流就没有
/// 回头路了（重建会让客户端收到重复内容）。
async fn open_chat(
    state: &ApiState,
    model: &str,
    messages: &[doubao::ChatMessage],
) -> std::result::Result<reqwest::Response, OpenError> {
    let targets = load_targets(&state.db)
        .await
        .map_err(|e| OpenError::Upstream(e.message))?;
    if targets.is_empty() {
        return Err(OpenError::NoAccount);
    }
    let start = state.rr.fetch_add(1, Ordering::Relaxed) % targets.len();
    let mut model_missing = true;
    let mut last_err = String::new();
    for step in 0..targets.len() {
        let target = &targets[(start + step) % targets.len()];
        let catalog = match resolve_catalog(state, target).await {
            Ok(c) => c,
            Err(e) => {
                last_err = format!("账号 {} 目录不可用：{}", target.label, e.message);
                continue;
            }
        };
        let Some(info) = catalog.models.get(model).cloned() else {
            // 这个账号没开这个模型：换下一个，全部都没有才回 404
            last_err = format!("账号 {} 没有模型 {}", target.label, model);
            continue;
        };
        model_missing = false;
        let payload = doubao::chat_payload(&catalog.bot, &info, messages);
        match send_chat(&state.client, &target.session, &payload).await {
            Ok(resp) => {
                // 哪个号承载了这次请求：排查额度去向时最常问的一句
                crate::app::log::log(
                    crate::app::log::Level::Debug,
                    "model_api",
                    format!("对话由账号 {} 承载", target.label),
                );
                if step > 0 {
                    crate::app::log::log(
                        crate::app::log::Level::Warn,
                        "model_api",
                        format!("对话顺延到账号 {}（第 {} 次尝试）", target.label, step + 1),
                    );
                }
                return Ok(resp);
            }
            Err(e) => {
                // 还没吐出任何字节，可以安全换下一个账号
                last_err = format!("账号 {} 对话失败：{}", target.label, e.message);
            }
        }
    }
    if model_missing {
        Err(OpenError::ModelNotFound)
    } else {
        Err(OpenError::Upstream(last_err))
    }
}

// ------------------------------------------------------------------ 增量流

/// 上游解出来的一个增量（或一次中断）。
enum StreamItem {
    Delta(sse::Delta),
    Error(String),
}

/// 把豆包 SSE 解码成增量流。三种接口形态共用这一条管道，只是最后的编码不同。
fn spawn_deltas(resp: reqwest::Response) -> tokio::sync::mpsc::Receiver<StreamItem> {
    let (tx, rx) = tokio::sync::mpsc::channel::<StreamItem>(64);
    tauri::async_runtime::spawn(async move {
        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut decoder = sse::SseDecoder::new();
        let mut size = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(_) => {
                    let _ = tx.send(StreamItem::Error("豆包流式响应中断".into())).await;
                    return;
                }
            };
            let text = String::from_utf8_lossy(chunk.as_ref());
            let deltas = match decoder.feed(&text) {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(StreamItem::Error(e.message)).await;
                    return;
                }
            };
            for d in deltas {
                size += d.text().len();
                if size > MAX_OUTPUT_BYTES {
                    let _ = tx.send(StreamItem::Error("输出超过大小限制".into())).await;
                    return;
                }
                if tx.send(StreamItem::Delta(d)).await.is_err() {
                    return; // 客户端已断开，没必要继续读上游
                }
            }
        }
    });
    rx
}

/// 收完整段回复（非流式接口用）。
async fn collect_text(
    mut rx: tokio::sync::mpsc::Receiver<StreamItem>,
) -> std::result::Result<(String, String), String> {
    let mut content = String::new();
    let mut reasoning = String::new();
    while let Some(item) = rx.recv().await {
        match item {
            StreamItem::Delta(sse::Delta::Content(t)) => content.push_str(&t),
            StreamItem::Delta(sse::Delta::Reasoning(t)) => reasoning.push_str(&t),
            StreamItem::Error(e) => return Err(e),
        }
    }
    Ok((content, reasoning))
}

// ------------------------------------------------------------------ 响应组装

fn sse_response(
    rx: tokio::sync::mpsc::Receiver<std::result::Result<Bytes, Infallible>>,
) -> Response {
    let body = Body::from_stream(ReceiverStream::new(rx));
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap()
}

/// OpenAI Chat Completions 的流式输出。
fn chat_stream(
    delta_rx: tokio::sync::mpsc::Receiver<StreamItem>,
    model: &str,
    id: String,
    created: i64,
) -> Response {
    let (tx, out) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, Infallible>>(64);
    let model = model.to_string();
    tauri::async_runtime::spawn(async move {
        let mut delta_rx = delta_rx;
        let _ = tx
            .send(Ok(encode_chunk(
                &id,
                created,
                &model,
                Some(json!({ "role": "assistant" })),
                None,
            )))
            .await;
        while let Some(item) = delta_rx.recv().await {
            match item {
                StreamItem::Delta(sse::Delta::Content(t)) => {
                    let _ = tx
                        .send(Ok(encode_chunk(
                            &id,
                            created,
                            &model,
                            Some(json!({ "content": t })),
                            None,
                        )))
                        .await;
                }
                StreamItem::Delta(sse::Delta::Reasoning(t)) => {
                    let _ = tx
                        .send(Ok(encode_chunk(
                            &id,
                            created,
                            &model,
                            Some(json!({ "reasoning_content": t })),
                            None,
                        )))
                        .await;
                }
                StreamItem::Error(e) => {
                    let _ = tx
                        .send(Ok(encode_error_chunk(&id, created, &model, &e)))
                        .await;
                    return;
                }
            }
        }
        let _ = tx
            .send(Ok(encode_chunk(
                &id,
                created,
                &model,
                Some(json!({})),
                Some("stop"),
            )))
            .await;
        let _ = tx.send(Ok(Bytes::from_static(b"data: [DONE]\n\n"))).await;
    });
    sse_response(out)
}

fn chat_completion(
    id: &str,
    created: i64,
    model: &str,
    content: &str,
    reasoning: &str,
) -> Response {
    let mut message = json!({ "role": "assistant", "content": content });
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    Json(json!({
        "id": id, "object": "chat.completion", "created": created, "model": model,
        "choices": [{ "index": 0, "message": message, "finish_reason": "stop" }]
    }))
    .into_response()
}

fn anthropic_sse(event: &str, data: Value) -> Bytes {
    Bytes::from(format!(
        "event: {event}\ndata: {}\n\n",
        serde_json::to_string(&data).unwrap_or_default()
    ))
}

/// HTTP 状态 → Anthropic 的错误类型名。
fn anthropic_kind(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "authentication_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::BAD_REQUEST => "invalid_request_error",
        s if s.as_u16() >= 500 => "api_error",
        _ => "invalid_request_error",
    }
}

fn anthropic_error(status: StatusCode, kind: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "type": "error", "error": { "type": kind, "message": message } })),
    )
        .into_response()
}

fn anthropic_message(id: &str, model: &str, text: &str) -> Value {
    json!({
        "id": id, "type": "message", "role": "assistant", "model": model,
        "content": [{ "type": "text", "text": text }],
        "stop_reason": "end_turn", "stop_sequence": null,
        "usage": { "input_tokens": 0, "output_tokens": est_tokens(text.chars().count()) }
    })
}

fn anthropic_stream(
    delta_rx: tokio::sync::mpsc::Receiver<StreamItem>,
    model: &str,
    id: String,
) -> Response {
    let (tx, out) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, Infallible>>(64);
    let model = model.to_string();
    tauri::async_runtime::spawn(async move {
        let mut delta_rx = delta_rx;
        let mut chars = 0usize;
        let _ = tx
            .send(Ok(anthropic_sse(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": id, "type": "message", "role": "assistant", "model": model,
                        "content": [], "stop_reason": null, "stop_sequence": null,
                        "usage": { "input_tokens": 0, "output_tokens": 0 }
                    }
                }),
            )))
            .await;
        let _ = tx
            .send(Ok(anthropic_sse(
                "content_block_start",
                json!({
                    "type": "content_block_start", "index": 0,
                    "content_block": { "type": "text", "text": "" }
                }),
            )))
            .await;
        while let Some(item) = delta_rx.recv().await {
            match item {
                StreamItem::Delta(sse::Delta::Content(t)) => {
                    chars += t.chars().count();
                    let _ = tx
                        .send(Ok(anthropic_sse(
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta", "index": 0,
                                "delta": { "type": "text_delta", "text": t }
                            }),
                        )))
                        .await;
                }
                // Anthropic 侧没开 thinking 就不该冒出 thinking 块，思维链在这里丢掉
                StreamItem::Delta(sse::Delta::Reasoning(_)) => {}
                StreamItem::Error(e) => {
                    let _ = tx
                        .send(Ok(anthropic_sse(
                            "error",
                            json!({
                                "type": "error",
                                "error": { "type": "api_error", "message": e }
                            }),
                        )))
                        .await;
                    return;
                }
            }
        }
        let _ = tx
            .send(Ok(anthropic_sse(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": 0 }),
            )))
            .await;
        let _ = tx
            .send(Ok(anthropic_sse(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                    "usage": { "output_tokens": est_tokens(chars) }
                }),
            )))
            .await;
        let _ = tx
            .send(Ok(anthropic_sse(
                "message_stop",
                json!({ "type": "message_stop" }),
            )))
            .await;
    });
    sse_response(out)
}

/// Responses 的一个输出条目（message 类型）。
fn response_item(item_id: &str, text: &str, status: &str) -> Value {
    let content = if status == "in_progress" {
        json!([])
    } else {
        json!([{ "type": "output_text", "text": text, "annotations": [] }])
    };
    json!({
        "id": item_id, "type": "message", "status": status, "role": "assistant",
        "content": content
    })
}

fn response_object(
    id: &str,
    item_id: &str,
    model: &str,
    created: i64,
    status: &str,
    text: &str,
) -> Value {
    let usage = est_tokens(text.chars().count());
    let output = if status == "in_progress" {
        json!([])
    } else {
        json!([response_item(item_id, text, "completed")])
    };
    json!({
        "id": id, "object": "response", "created_at": created, "model": model,
        "status": status, "output": output, "output_text": text,
        "error": null, "incomplete_details": null,
        "usage": { "input_tokens": 0, "output_tokens": usage, "total_tokens": usage }
    })
}

fn response_event(name: &str, data: Value) -> Bytes {
    Bytes::from(format!(
        "event: {name}\ndata: {}\n\n",
        serde_json::to_string(&data).unwrap_or_default()
    ))
}

fn responses_stream(
    delta_rx: tokio::sync::mpsc::Receiver<StreamItem>,
    model: &str,
    id: &str,
    item_id: &str,
    created: i64,
) -> Response {
    let (tx, out) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, Infallible>>(64);
    let model = model.to_string();
    let id = id.to_string();
    let item_id = item_id.to_string();
    tauri::async_runtime::spawn(async move {
        let mut delta_rx = delta_rx;
        let mut full = String::new();
        let _ = tx
            .send(Ok(response_event(
                "response.created",
                json!({
                    "type": "response.created",
                    "response": response_object(&id, &item_id, &model, created, "in_progress", "")
                }),
            )))
            .await;
        let _ = tx
            .send(Ok(response_event(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added", "output_index": 0,
                    "item": response_item(&item_id, "", "in_progress")
                }),
            )))
            .await;
        let _ = tx
            .send(Ok(response_event(
                "response.content_part.added",
                json!({
                    "type": "response.content_part.added",
                    "item_id": item_id, "output_index": 0, "content_index": 0,
                    "part": { "type": "output_text", "text": "", "annotations": [] }
                }),
            )))
            .await;
        while let Some(item) = delta_rx.recv().await {
            match item {
                StreamItem::Delta(sse::Delta::Content(t)) => {
                    full.push_str(&t);
                    let _ = tx
                        .send(Ok(response_event(
                            "response.output_text.delta",
                            json!({
                                "type": "response.output_text.delta",
                                "item_id": item_id, "output_index": 0, "content_index": 0,
                                "delta": t
                            }),
                        )))
                        .await;
                }
                // 思维链不进 output_text，避免客户端把它当正文显示
                StreamItem::Delta(sse::Delta::Reasoning(_)) => {}
                StreamItem::Error(e) => {
                    let _ = tx
                        .send(Ok(response_event(
                            "response.failed",
                            json!({
                                "type": "response.failed",
                                "response": {
                                    "id": id, "object": "response", "created_at": created,
                                    "model": model, "status": "failed", "output": [],
                                    "error": { "code": "upstream_error", "message": e }
                                }
                            }),
                        )))
                        .await;
                    return;
                }
            }
        }
        let _ = tx
            .send(Ok(response_event(
                "response.output_text.done",
                json!({
                    "type": "response.output_text.done",
                    "item_id": item_id, "output_index": 0, "content_index": 0,
                    "text": full
                }),
            )))
            .await;
        let _ = tx
            .send(Ok(response_event(
                "response.content_part.done",
                json!({
                    "type": "response.content_part.done",
                    "item_id": item_id, "output_index": 0, "content_index": 0,
                    "part": { "type": "output_text", "text": full, "annotations": [] }
                }),
            )))
            .await;
        let _ = tx
            .send(Ok(response_event(
                "response.output_item.done",
                json!({
                    "type": "response.output_item.done", "output_index": 0,
                    "item": response_item(&item_id, &full, "completed")
                }),
            )))
            .await;
        let _ = tx
            .send(Ok(response_event(
                "response.completed",
                json!({
                    "type": "response.completed",
                    "response": response_object(&id, &item_id, &model, created, "completed", &full)
                }),
            )))
            .await;
    });
    sse_response(out)
}

fn encode_chunk(
    id: &str,
    created: i64,
    model: &str,
    delta: Option<Value>,
    finish: Option<&str>,
) -> Bytes {
    let v = json!({
        "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
        "choices": [{ "index": 0, "delta": delta.unwrap_or(json!({})), "finish_reason": finish }]
    });
    Bytes::from(format!(
        "data: {}\n\n",
        serde_json::to_string(&v).unwrap_or_default()
    ))
}

fn encode_error_chunk(id: &str, _created: i64, _model: &str, message: &str) -> Bytes {
    Bytes::from(format!(
        "event: error\ndata: {}\n\n",
        serde_json::to_string(&json!({
            "error": { "message": message, "type": "upstream_error", "code": null, "request_id": id }
        }))
        .unwrap_or_default()
    ))
}

fn key_of(state: &ApiState) -> String {
    state.key.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 鉴权：OpenAI 侧用 `Authorization: Bearer`，Anthropic 侧习惯 `x-api-key`，两种都认。
fn authorize(headers: &HeaderMap, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim() == key)
        .unwrap_or(false);
    let x_api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim() == key)
        .unwrap_or(false);
    bearer || x_api_key
}

fn api_error(status: StatusCode, type_: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": { "message": message, "type": type_, "code": status.as_u16() }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_key_has_prefix_and_length() {
        let k = fresh_key();
        assert!(k.starts_with(KEY_PREFIX), "{k}");
        assert_eq!(k.len(), KEY_PREFIX.len() + 64);
    }

    #[test]
    fn authorize_checks_bearer() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer sk-am-abc".parse().unwrap());
        assert!(authorize(&h, "sk-am-abc"));
        assert!(!authorize(&h, "sk-am-other"));
        assert!(!authorize(&h, ""));
    }

    #[test]
    fn authorize_accepts_anthropic_style_key() {
        let mut h = HeaderMap::new();
        h.insert("x-api-key", "sk-am-abc".parse().unwrap());
        assert!(authorize(&h, "sk-am-abc"), "Anthropic 客户端只发 x-api-key");
        assert!(!authorize(&h, "sk-am-other"));
    }

    #[test]
    fn parse_anthropic_maps_system_and_content_blocks() {
        let body = json!({
            "model": "seed-lite-7b",
            "max_tokens": 64,
            "system": "你是助手",
            "stream": true,
            "messages": [
                { "role": "user", "content": [
                    { "type": "text", "text": "第一段" },
                    { "type": "text", "text": "第二段" }
                ] }
            ]
        });
        let req = parse_anthropic(&body).unwrap();
        assert_eq!(req.model, "seed-lite-7b");
        assert!(req.stream);
        assert_eq!(req.messages.len(), 2, "system 要转成一条消息");
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(
            req.messages[1].content, "第一段\n第二段",
            "多个文本块按行拼接"
        );

        // 图片块没接入，必须明确报错而不是悄悄丢内容
        let with_image = json!({
            "model": "m",
            "messages": [{ "role": "user", "content": [
                { "type": "image", "source": {} }
            ] }]
        });
        assert!(parse_anthropic(&with_image).is_err());
    }

    #[test]
    fn parse_anthropic_rejects_tools() {
        let body = json!({
            "model": "m",
            "tools": [{ "name": "t" }],
            "messages": [{ "role": "user", "content": "hi" }]
        });
        assert!(
            parse_anthropic(&body).is_err(),
            "工具调用尚未接入，必须明确拒绝"
        );
    }

    #[test]
    fn parse_responses_accepts_string_and_items() {
        let plain = json!({ "model": "m", "input": "你好" });
        let req = parse_responses(&plain).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert!(!req.stream);

        let items = json!({
            "model": "m",
            "instructions": "简洁回答",
            "input": [
                { "type": "message", "role": "user", "content": [
                    { "type": "input_text", "text": "问题" }
                ] }
            ]
        });
        let req = parse_responses(&items).unwrap();
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.messages[1].content, "问题");

        let no_input = json!({ "model": "m" });
        assert!(parse_responses(&no_input).is_err());
    }

    #[test]
    fn anthropic_and_responses_payloads_have_expected_shape() {
        let msg = anthropic_message("msg_1", "m", "你好");
        assert_eq!(msg["type"], "message");
        assert_eq!(msg["content"][0]["type"], "text");
        assert_eq!(msg["stop_reason"], "end_turn");

        let resp = response_object("resp_1", "msg_1", "m", 1, "completed", "你好");
        assert_eq!(resp["object"], "response");
        assert_eq!(resp["status"], "completed");
        assert_eq!(resp["output"][0]["content"][0]["type"], "output_text");
        assert_eq!(resp["output_text"], "你好");

        // 进行中的 response 不应带半截正文
        let running = response_object("resp_1", "msg_1", "m", 1, "in_progress", "");
        assert_eq!(running["output"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn parse_chat_accepts_text_and_rejects_tools() {
        let ok = json!({ "model": "m", "messages": [{"role":"user","content":"hi"}] });
        let req = parse_chat(&ok).unwrap();
        assert!(!req.stream);
        assert_eq!(req.messages.len(), 1);

        let tools =
            json!({ "model": "m", "messages": [{"role":"user","content":"hi"}], "tools": [] });
        assert!(parse_chat(&tools).is_err());

        let no_user = json!({ "model": "m", "messages": [{"role":"assistant","content":"hi"}] });
        assert!(parse_chat(&no_user).is_err());
    }

    #[test]
    fn chunk_encoding_matches_openai_shape() {
        let b = encode_chunk("chatcmpl-1", 1, "m", Some(json!({"content":"hi"})), None);
        let text = String::from_utf8_lossy(&b);
        assert!(text.starts_with("data: "), "{text}");
        let v: Value = serde_json::from_str(text.trim_start_matches("data: ").trim()).unwrap();
        assert_eq!(v["object"], "chat.completion.chunk");
        assert_eq!(v["choices"][0]["delta"]["content"], "hi");
    }

    /// 端口类用例会「先占后放」，并行跑会互相抢端口，串起来执行。
    static PORT_LOCK: Mutex<()> = Mutex::new(());

    fn port_guard() -> std::sync::MutexGuard<'static, ()> {
        PORT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // 端口用例必须串行，锁故意跨 await 持有
    async fn bind_uses_configured_port_when_free() {
        let _g = port_guard();
        // 先占一个随机端口再释放，拿到一个当下大概率空闲的端口
        let probe = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let (listener, actual) = bind(port).await.expect("空闲端口应直接用");
        assert_eq!(actual, port, "端口空闲时不该换");
        drop(listener);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // 端口用例必须串行，锁故意跨 await 持有
    async fn bind_falls_back_to_neighbor_port_when_taken() {
        let _g = port_guard();
        // 占住端口模拟「被别的程序用了」
        let taken = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = taken.local_addr().unwrap().port();

        let (listener, actual) = bind(port).await.expect("占用时应顺延到相邻端口，不该报错");
        assert_ne!(actual, port, "配置端口被占用时必须换一个端口");
        assert!(
            actual > port && actual <= port.saturating_add(PORT_PROBE_SPAN),
            "应优先顺延到相邻端口，实际 {actual}（配置 {port}）"
        );
        drop(listener);
    }

    /// 多账号：所有「启用 + 有凭据」的豆包账号都要进目标列表，停用的、别的平台、空凭据的都要排除。
    #[tokio::test]
    async fn load_targets_includes_every_enabled_doubao_account() {
        use crate::app::accounts::view::AccountIdentity;
        use crate::app::db::Db;

        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());

        let seed = |iid: &str, platform: &str, name: &str, cookie: &str, enabled: bool| {
            let identity = AccountIdentity {
                iid: iid.into(),
                platform: platform.into(),
                platform_label: platform.into(),
                account_id: "u1".into(),
                name: name.into(),
                expires_at: None,
                source: "test".into(),
                requires_checkin: false,
                has_secret: true,
            };
            repo::upsert_identity(&db, &identity, 0).unwrap();
            let cred = Credential {
                token: cookie.into(),
                refresh_token: String::new(),
                expires_at: String::new(),
                meta: serde_json::Map::new(),
            };
            repo::set_secret(&db, iid, &cred.to_bytes().unwrap(), None).unwrap();
            repo::set_enabled(&db, iid, enabled).unwrap();
        };

        seed(
            "doubao:a",
            "doubao",
            "豆包 A",
            "sessionid=aaa; s_v_web_id=fp-a",
            true,
        );
        seed(
            "doubao:b",
            "doubao",
            "豆包 B",
            "sessionid=bbb; s_v_web_id=fp-b",
            true,
        );
        seed("doubao:off", "doubao", "停用的", "sessionid=ccc", false);
        seed("doubao:empty", "doubao", "空凭据", "   ", true);
        seed("workbuddy:w", "workbuddy", "别的平台", "token-xyz", true);

        let targets = load_targets(&db).await.unwrap();
        let iids: Vec<&str> = targets.iter().map(|t| t.iid.as_str()).collect();
        assert_eq!(
            iids,
            vec!["doubao:a", "doubao:b"],
            "只收启用且有凭据的豆包账号"
        );
        assert_eq!(targets[0].label, "豆包 A", "报错/日志用账号名而不是 iid");
        assert_eq!(targets[0].session.fp, "fp-a", "s_v_web_id 要带进会话");
        assert!(
            targets[0].session.seed != targets[1].session.seed,
            "两个账号的种子必须不同，否则被豆包认成同一台设备"
        );
    }
}
