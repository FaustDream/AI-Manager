//! 豆包浏览器登录：起一个**临时 Chrome/Edge 会话**，用户在窗口里登录，
//! 后台通过 CDP 轮询拿到 cookie，再按身份自动入库。
//!
//! 与「粘贴 Cookie」(`accounts::cookie`) 的区别：
//!   - 粘贴 Cookie 要用户自己开 DevTools 复制，门槛高、容易复制错域；
//!   - 这里用 `--user-data-dir=<临时目录>` + `--remote-debugging-port=0`，
//!     与日常浏览器完全隔离——日常 Chrome/Edge 的登录态、标签页都不受影响。
//!
//! **身份去重**：登录后调 `/passport/account/info/v2/` 拿 `user_id + 企业类型`，
//! 同一身份 → 更新原账号；新身份 → 新增一行。这比「按 cookie 指纹」可靠——
//! cookie 会随会话刷新变化，身份才是账号真正的锚点。
//!
//! **取 cookie 走 CDP（`Network.getCookies`）**，拿到的是浏览器解密后的明文；
//! 不直接读临时 profile 里的加密 cookie 库（Chrome 127+ 是 v20 应用绑定加密，
//! 现有 DPAPI+AES-GCM 只解得了 v10）。
//!
//! 为什么用 `--remote-debugging-port=0` 而不是参考项目的 `--remote-debugging-pipe`：
//! 后者要把句柄 3/4 通过 `lpReserved2` 句柄表传给子进程（MSVCRT 的 fd 继承机制），
//! `std::process::Command` 不暴露这条路，纯 Rust 复刻要一大段不安全的 CreateProcessW。
//! 随机端口 + 短命会话的风险足够低，用读 `DevToolsActivePort` + WebSocket 更稳。

use crate::app::accounts::credential::Credential;
use crate::app::accounts::{login, repo, AccountIdentity};
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest, RealHttp};
use crate::app::timeutil::now_ms;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async, WebSocketStream};

/// 登录页（CDP 里只盯这个域）。
const LOGIN_URL: &str = "https://www.doubao.com/chat/";
/// 身份接口：拿到 `user_id` + 企业类型，作为账号去重键。
const IDENTITY_URL: &str = "https://www.doubao.com/passport/account/info/v2/\
    ?account_sdk_source=web&sdk_version=2.2.5&aid=582478&real_aid=582478\
    &device_platform=web&samantha_web=1&use-olympus-account=1";
const CLIENT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/147.0.7727.149 Safari/537.36";
/// cookie 轮询间隔
const POLL_MS: u64 = 2000;
/// 整个登录会话的时限：够用户扫码/输密码
const TIMEOUT_MS: i64 = 10 * 60 * 1000;
/// 等浏览器写 `DevToolsActivePort` 的时限
const PORT_TIMEOUT_MS: i64 = 15 * 1000;

/// 给界面展示浏览器列表用。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct BrowserInfo {
    pub id: String,
    pub name: String,
}

/// 登录会话的当前状态，前端据此渲染弹窗。事件名 `doubao:login`。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct BrowserLoginStatus {
    /// idle / starting / waiting / importing / success / error / cancelled
    pub state: String,
    pub message: String,
    pub browser_name: String,
    pub account_name: String,
    pub updated: bool,
    pub active: bool,
}

const EVENT: &str = "doubao:login";

fn idle_status() -> BrowserLoginStatus {
    BrowserLoginStatus {
        state: "idle".into(),
        message: "在浏览器完成登录后，将自动接入本机账号列表。".into(),
        browser_name: String::new(),
        account_name: String::new(),
        updated: false,
        active: false,
    }
}

/// 登录会话的全局单例：同一时间只允许一个浏览器登录窗口。
/// 用 OnceLock 而不是塞进 AppState——这里和「事件总线」一样与窗口/DB 无关，
/// 任何调用深度都能拿到（对齐 events 的 ARCH-12 白名单做法）。
struct Manager {
    active: AtomicBool,
    cancel: AtomicBool,
    status: Mutex<BrowserLoginStatus>,
}

fn manager() -> &'static Manager {
    static M: OnceLock<Manager> = OnceLock::new();
    M.get_or_init(|| Manager {
        active: AtomicBool::new(false),
        cancel: AtomicBool::new(false),
        status: Mutex::new(idle_status()),
    })
}

fn set_status(status: BrowserLoginStatus) {
    let m = manager();
    if let Ok(mut slot) = m.status.lock() {
        *slot = status.clone();
    }
    crate::app::events::emit(EVENT, &status);
}

pub fn status() -> BrowserLoginStatus {
    manager()
        .status
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| idle_status())
}

/// 取消进行中的登录会话（用户点「取消」或退出登录页时用）。
pub fn cancel() -> BrowserLoginStatus {
    let m = manager();
    m.cancel.store(true, Ordering::SeqCst);
    let s = BrowserLoginStatus {
        state: "cancelled".into(),
        message: "本次浏览器登录已取消，已有账号不受影响。".into(),
        browser_name: String::new(),
        account_name: String::new(),
        updated: false,
        active: false,
    };
    set_status(s.clone());
    s
}

/// 发起一次浏览器登录。返回的只是「已开始」的即时状态，后续进度走 `doubao:login` 事件。
/// `browser_id` 为空时用第一个探测到的浏览器。
pub fn start(db: Arc<Db>, browser_id: Option<String>) -> Result<BrowserLoginStatus> {
    let m = manager();
    // 已经有会话在跑：直接回当前状态，不重复开窗口（与参考项目的 409 语义一致）
    if m.active.swap(true, Ordering::SeqCst) {
        return Ok(status());
    }
    m.cancel.store(false, Ordering::SeqCst);
    set_status(BrowserLoginStatus {
        state: "starting".into(),
        message: "正在准备独立浏览器会话…".into(),
        browser_name: String::new(),
        account_name: String::new(),
        updated: false,
        active: true,
    });

    crate::app::runtime::runtime().spawn(async move {
        run(db, browser_id).await;
    });
    Ok(status())
}

/// 会话的后半段：跑完整条「探测 → 启动 → 轮询 → 核验 → 入库 → 清理」，
/// 结束后把 active 复位，并按结果推事件。
async fn run(db: Arc<Db>, browser_id: Option<String>) {
    let m = manager();
    let result = run_inner(&db, browser_id.as_deref()).await;
    let cancelled = m.cancel.load(Ordering::SeqCst);
    m.cancel.store(false, Ordering::SeqCst);
    m.active.store(false, Ordering::SeqCst);

    if cancelled {
        // 取消单独走一条状态，别和「登录失败」混成同一种红字
        set_status(BrowserLoginStatus {
            state: "cancelled".into(),
            message: "本次浏览器登录已取消，已有账号不受影响。".into(),
            browser_name: String::new(),
            account_name: String::new(),
            updated: false,
            active: false,
        });
        return;
    }

    match result {
        Ok(imported) => {
            // 复用既有登录链路的事件：前端监听 account:login 会刷新账号列表
            crate::app::events::emit("account:login", &imported.outcome);
            set_status(BrowserLoginStatus {
                state: "success".into(),
                message: imported.outcome.message.clone(),
                browser_name: String::new(),
                account_name: imported.account_name,
                updated: imported.updated,
                active: false,
            });
        }
        Err(e) => {
            crate::app::log::fail(
                "accounts",
                "doubao_browser_login",
                format!("豆包浏览器登录失败：{}", e.message),
                e.evidence(),
            );
            set_status(BrowserLoginStatus {
                state: "error".into(),
                message: e.message,
                browser_name: String::new(),
                account_name: String::new(),
                updated: false,
                active: false,
            });
        }
    }
}

struct Imported {
    outcome: login::LoginOutcome,
    account_name: String,
    updated: bool,
}

async fn run_inner(db: &Db, browser_id: Option<&str>) -> Result<Imported> {
    let browsers = discover_browsers();
    let browser = match browser_id {
        Some(id) => browsers.iter().find(|b| b.id == id),
        None => browsers.first(),
    }
    .cloned()
    .ok_or_else(|| {
        AppError::new(
            ErrorKind::Io,
            "没有找到可用的 Chrome / Edge，请安装后再试（或改用「粘贴 Cookie」添加）",
        )
    })?;

    let profile = TempProfile::new()?;
    let mut child = launch_browser(&browser, profile.path())?;
    set_status(BrowserLoginStatus {
        state: "waiting".into(),
        message: format!(
            "已打开一个独立的 {} 窗口，请在窗口里登录豆包；登录后会自动接入，无需复制 Cookie。",
            browser.name
        ),
        browser_name: browser.name.clone(),
        account_name: String::new(),
        updated: false,
        active: true,
    });

    // 拿不到调试端口/连不上 CDP 时，先把浏览器收掉再报错
    let ws_url = match read_devtools_port(profile.path()).await {
        Ok(url) => url,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    let mut cdp = match CdpClient::connect(&ws_url).await {
        Ok(cdp) => cdp,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };

    // 主流程：轮询 cookie → 稳定后解析身份 → 入库
    let result = poll_and_import(db, &mut cdp).await;

    // 收尾（无论成败）：优雅关 → 硬杀 → 删临时目录。
    // Browser.close 可能不回响应（浏览器收到就关），超时兜底，别让它拖慢清理。
    let _ = tokio::time::timeout(
        Duration::from_millis(800),
        cdp.call("Browser.close", json!({}), None),
    )
    .await;
    let _ = tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = child.kill();
    let _ = child.wait();
    drop(cdp);
    drop(profile);

    result
}

async fn poll_and_import(db: &Db, cdp: &mut CdpClient) -> Result<Imported> {
    let m = manager();
    let deadline = now_ms() + TIMEOUT_MS;
    let mut stable = String::new();

    loop {
        if m.cancel.load(Ordering::SeqCst) {
            return Err(AppError::new(ErrorKind::Cancelled, "已取消"));
        }
        if now_ms() >= deadline {
            return Err(AppError::new(ErrorKind::Io, "登录超时（10 分钟），请重试"));
        }

        let cookies = read_cookies(cdp).await.unwrap_or_default();
        if !has_session(&cookies) {
            // 还没登录：清零指纹，继续等
            stable.clear();
            tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
            continue;
        }
        let fingerprint = session_fingerprint(&cookies);
        if fingerprint != stable {
            // 指纹还在变（cookie 正在写）：再观察一轮
            stable = fingerprint;
            tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
            continue;
        }

        // 指纹连续两轮一致 → 登录态稳定，开始核验身份并入库
        set_status(BrowserLoginStatus {
            state: "importing".into(),
            message: "检测到登录，正在核验账号并保存…".into(),
            browser_name: String::new(),
            account_name: String::new(),
            updated: false,
            active: true,
        });
        let header = cookie_header(&cookies);
        let (user_id, enterprise, nickname) = resolve_identity(&header).await?;
        let imported = import(db, &header, &user_id, &enterprise, &nickname)?;
        return Ok(imported);
    }
}

/// 读 CDP `Network.getCookies`。拿不到（窗口被关、页面还没到豆包）就返回空列表，
/// 由上层继续轮询，而不是当成致命错误。
async fn read_cookies(cdp: &mut CdpClient) -> Result<Vec<Value>> {
    let targets = cdp.call("Target.getTargets", json!({}), None).await?;
    let infos = targets
        .get("targetInfos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let page = infos.iter().find(|t| {
        t.get("type").and_then(Value::as_str) == Some("page")
            && t.get("url")
                .and_then(Value::as_str)
                .map(is_doubao_url)
                .unwrap_or(false)
    });
    let Some(page) = page else {
        return Ok(Vec::new());
    };
    let target_id = page
        .get("targetId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if target_id.is_empty() {
        return Ok(Vec::new());
    }
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
            None,
        )
        .await?;
    let session_id = attached
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if session_id.is_empty() {
        return Ok(Vec::new());
    }
    let resp = cdp
        .call(
            "Network.getCookies",
            json!({ "urls": [LOGIN_URL] }),
            Some(&session_id),
        )
        .await?;
    Ok(resp
        .get("cookies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// 调身份接口，返回 `(user_id, 企业类型, 昵称)`。
async fn resolve_identity(cookie: &str) -> Result<(String, String, String)> {
    let http = RealHttp::new();
    let mut req = HttpRequest::get(IDENTITY_URL);
    req.headers = vec![
        ("Cookie".into(), cookie.to_string()),
        ("User-Agent".into(), CLIENT_UA.to_string()),
        ("Referer".into(), "https://www.doubao.com/chat/".into()),
        ("Accept".into(), "application/json, text/plain, */*".into()),
    ];
    let resp = http.send(req).await?;
    let body = resp.json()?;
    let data = body.get("data").cloned().unwrap_or_else(|| json!({}));
    parse_identity(&data).ok_or_else(|| {
        AppError::new(
            ErrorKind::AuthExpired,
            "没能从豆包拿到账号身份，请确认已在窗口里完成登录",
        )
    })
}

/// 按身份入库：同身份覆盖原账号，新身份新增一行。
fn import(
    db: &Db,
    cookie: &str,
    user_id: &str,
    enterprise: &str,
    nickname: &str,
) -> Result<Imported> {
    let account_id = account_id_of(user_id, enterprise);
    let iid = format!("doubao:{account_id}");
    let existed = repo::identity(db, &iid)
        .map(|o| o.is_some())
        .unwrap_or(false);

    // 与「粘贴 Cookie」写入同一套凭据结构：token 是整段 cookie，
    // seed 决定设备标识（同一账号每次算出同一组 web_id / tea_uuid）。
    let mut cred = Credential {
        token: cookie.to_string(),
        ..Default::default()
    };
    cred.put("seed", json!(account_id));
    cred.put("uid", json!(account_id));
    cred.put("doubao_user_id", json!(user_id));
    cred.put("doubao_enterprise", json!(enterprise));

    let identity = AccountIdentity {
        iid: iid.clone(),
        platform: "doubao".into(),
        platform_label: "豆包".into(),
        account_id,
        name: nickname.to_string(),
        expires_at: None,
        source: "浏览器登录".into(),
        requires_checkin: true,
        has_secret: true,
    };
    let outcome = login::save(db, identity, &cred, None, nickname)?;
    Ok(Imported {
        outcome,
        account_name: nickname.to_string(),
        updated: existed,
    })
}

// ------------------------------------------------------------------ 探测与启动

/// 探测 Chrome / Edge 的安装位置。只看 exe 在不在，绝不碰用户的日常浏览器 profile。
pub fn discover_browsers() -> Vec<BrowserInfo> {
    let pf = std::env::var("ProgramFiles").ok().map(PathBuf::from);
    let pfx86 = std::env::var("ProgramFiles(x86)").ok().map(PathBuf::from);
    let local = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);

    let chrome = [
        pf.as_ref()
            .map(|p| p.join("Google/Chrome/Application/chrome.exe")),
        pfx86
            .as_ref()
            .map(|p| p.join("Google/Chrome/Application/chrome.exe")),
        local
            .as_ref()
            .map(|p| p.join("Google/Chrome/Application/chrome.exe")),
    ];
    let edge = [
        pfx86
            .as_ref()
            .map(|p| p.join("Microsoft/Edge/Application/msedge.exe")),
        pf.as_ref()
            .map(|p| p.join("Microsoft/Edge/Application/msedge.exe")),
    ];

    let mut out = Vec::new();
    if chrome.into_iter().flatten().any(|p| p.exists()) {
        out.push(BrowserInfo {
            id: "chrome".into(),
            name: "Chrome".into(),
        });
    }
    if edge.into_iter().flatten().any(|p| p.exists()) {
        out.push(BrowserInfo {
            id: "edge".into(),
            name: "Edge".into(),
        });
    }
    out
}

/// 启动临时会话。`DETACHED_PROCESS`：不继承本进程控制台（见 `process` 的说明）。
fn launch_browser(browser: &BrowserInfo, profile: &Path) -> Result<std::process::Child> {
    let exe = browser_exe(&browser.id)
        .ok_or_else(|| AppError::new(ErrorKind::Io, "没有找到浏览器安装位置"))?;
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    Command::new(&exe)
        .arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-mode")
        .arg("--remote-allow-origins=*")
        .arg("--new-window")
        .arg(LOGIN_URL)
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("启动浏览器失败：{e}")))
}

fn browser_exe(id: &str) -> Option<PathBuf> {
    let pf = std::env::var("ProgramFiles").ok().map(PathBuf::from);
    let pfx86 = std::env::var("ProgramFiles(x86)").ok().map(PathBuf::from);
    let local = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let candidates: Vec<Option<PathBuf>> = match id {
        "edge" => vec![
            pfx86.map(|p| p.join("Microsoft/Edge/Application/msedge.exe")),
            pf.map(|p| p.join("Microsoft/Edge/Application/msedge.exe")),
        ],
        _ => vec![
            pf.map(|p| p.join("Google/Chrome/Application/chrome.exe")),
            pfx86.map(|p| p.join("Google/Chrome/Application/chrome.exe")),
            local.map(|p| p.join("Google/Chrome/Application/chrome.exe")),
        ],
    };
    candidates.into_iter().flatten().find(|p| p.exists())
}

/// 临时 profile 目录：退出即删，绝不留登录残留。
struct TempProfile(PathBuf);

impl TempProfile {
    fn new() -> Result<Self> {
        let base = std::env::temp_dir().join("ai-manager-doubao");
        std::fs::create_dir_all(&base)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("创建临时目录失败：{e}")))?;
        let dir = base.join(format!("login-{}-{}", now_ms(), rand_suffix()));
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("创建临时目录失败：{e}")))?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempProfile {
    fn drop(&mut self) {
        // Chrome 的渲染进程在主进程退出后要一小会儿才释放文件锁，删不掉就重试几次。
        for _ in 0..5 {
            if std::fs::remove_dir_all(&self.0).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn rand_suffix() -> String {
    let mut buf = [0u8; 8];
    let _ = getrandom::getrandom(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------ CDP over WebSocket

struct CdpClient {
    ws: WebSocketStream<TcpStream>,
    next_id: u64,
}

impl CdpClient {
    async fn connect(url: &str) -> Result<Self> {
        let request = url
            .into_client_request()
            .map_err(|e| AppError::new(ErrorKind::Io, format!("调试地址无效：{e}")))?;
        let port = parse_ws_port(url)?;
        let stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|e| AppError::new(ErrorKind::Io, format!("连不上浏览器调试端口：{e}")))?;
        let (ws, _) = client_async(request, stream)
            .await
            .map_err(|e| AppError::new(ErrorKind::Io, format!("WebSocket 握手失败：{e}")))?;
        Ok(Self { ws, next_id: 0 })
    }

    /// 发一条 CDP 命令并等对应 id 的响应；事件（无 id）直接跳过。
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            msg["sessionId"] = json!(sid);
        }
        self.ws
            .send(Message::Text(msg.to_string().into()))
            .await
            .map_err(|e| AppError::new(ErrorKind::Io, format!("浏览器连接已断开：{e}")))?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let item = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .map_err(|_| AppError::new(ErrorKind::Io, "浏览器响应超时"))?
                .ok_or_else(|| AppError::new(ErrorKind::Io, "浏览器连接已断开"))?
                .map_err(|e| AppError::new(ErrorKind::Io, format!("浏览器通信失败：{e}")))?;
            if let Message::Text(text) = item {
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                if v.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(err) = v.get("error") {
                    let msg = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("CDP 错误");
                    return Err(AppError::new(
                        ErrorKind::Io,
                        format!("浏览器调试失败：{msg}"),
                    ));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }
}

fn parse_ws_port(url: &str) -> Result<u16> {
    let rest = url
        .strip_prefix("ws://127.0.0.1:")
        .or_else(|| url.strip_prefix("ws://localhost:"))
        .ok_or_else(|| AppError::new(ErrorKind::Io, "调试地址不是本机 ws"))?;
    rest.split('/')
        .next()
        .unwrap_or("")
        .parse::<u16>()
        .map_err(|_| AppError::new(ErrorKind::Io, "调试端口无效"))
}

/// 读 `<profile>/DevToolsActivePort`：第一行端口、第二行浏览器 WebSocket 路径。
async fn read_devtools_port(profile: &Path) -> Result<String> {
    let path = profile.join("DevToolsActivePort");
    let deadline = now_ms() + PORT_TIMEOUT_MS;
    loop {
        if now_ms() >= deadline {
            return Err(AppError::new(
                ErrorKind::Io,
                "浏览器调试端口一直没就绪，请重试",
            ));
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut lines = text.lines();
            let port = lines.next().unwrap_or("").trim().to_string();
            let ws_path = lines.next().unwrap_or("").trim().to_string();
            if !port.is_empty() && !ws_path.is_empty() {
                return Ok(format!("ws://127.0.0.1:{port}{ws_path}"));
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ------------------------------------------------------------------ 纯函数

/// 目标页是不是豆包页（`Network.getCookies` 只在该页上取）。
fn is_doubao_url(url: &str) -> bool {
    url.starts_with("https://www.doubao.com/") || url.starts_with("https://doubao.com/")
}

/// 筛出豆包根域、路径 `/` 的合法 cookie（`name, value`）。
fn doubao_cookies(cookies: &[Value]) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    for c in cookies {
        let domain = c
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_start_matches('.');
        if domain != "doubao.com" && domain != "www.doubao.com" {
            continue;
        }
        if c.get("path").and_then(Value::as_str) != Some("/") {
            continue;
        }
        let (Some(name), Some(value)) = (
            c.get("name").and_then(Value::as_str),
            c.get("value").and_then(Value::as_str),
        ) else {
            continue;
        };
        if name.is_empty()
            || name.len() > 256
            || name
                .chars()
                .any(|ch| ch.is_ascii_control() || matches!(ch, '=' | ';' | ',' | ' '))
        {
            continue;
        }
        out.push((name, value));
    }
    out
}

/// 是否有会话标记（`sessionid` / `sessionid_ss`）。
fn has_session(cookies: &[Value]) -> bool {
    doubao_cookies(cookies)
        .iter()
        .any(|(n, v)| !v.is_empty() && (*n == "sessionid" || *n == "sessionid_ss"))
}

/// 全部合法 cookie 拼成 `a=b; c=d`。
fn cookie_header(cookies: &[Value]) -> String {
    doubao_cookies(cookies)
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 会话指纹：只取 sessionid / sessionid_ss，排序后 SHA256。连续两轮一致才算稳定。
fn session_fingerprint(cookies: &[Value]) -> String {
    let mut pairs: Vec<String> = doubao_cookies(cookies)
        .iter()
        .filter(|(n, _)| *n == "sessionid" || *n == "sessionid_ss")
        .map(|(n, v)| format!("{n}={v}"))
        .collect();
    pairs.sort();
    let digest = Sha256::digest(pairs.join(";").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 身份接口回包 → `(user_id, 企业类型, 昵称)`。优先无损字符串 `user_id_str`；
/// 只给大整数 `user_id` 时若 `serde_json` 装不下（超出 i64）就放弃，不拿不精确的 id 当键。
fn parse_identity(data: &Value) -> Option<(String, String, String)> {
    let user = data
        .get("user_id_str")
        .and_then(as_text)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            data.get("user_id")
                .and_then(as_text)
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            data.get("user_id")
                .and_then(Value::as_i64)
                .map(|n| n.to_string())
        })?;
    let enterprise = data
        .get("passport_enterprise_user_type")
        .and_then(as_text)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0".to_string());
    let name = data
        .get("name")
        .and_then(as_text)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "豆包账号".to_string());
    Some((user, enterprise, name))
}

fn as_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 账号主键：由 `user_id + 企业类型` 派生，同一身份永远落在同一行。
fn account_id_of(user_id: &str, enterprise: &str) -> String {
    let digest = Sha256::digest(format!("{user_id}:{enterprise}").as_bytes());
    let hex: String = digest.iter().take(5).map(|b| format!("{b:02x}")).collect();
    format!("db{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str, value: &str) -> Value {
        json!({ "name": name, "value": value, "domain": "www.doubao.com", "path": "/" })
    }

    #[test]
    fn filters_non_doubao_and_non_root_cookies() {
        let cookies = vec![
            cookie("sessionid", "abc"),
            json!({ "name": "x", "value": "1", "domain": "other.com", "path": "/" }),
            json!({ "name": "y", "value": "2", "domain": "www.doubao.com", "path": "/sub" }),
        ];
        let header = cookie_header(&cookies);
        assert_eq!(header, "sessionid=abc");
        assert!(has_session(&cookies));
    }

    #[test]
    fn fingerprint_only_uses_session_markers_and_is_stable() {
        let a = vec![
            cookie("sessionid", "s1"),
            cookie("sessionid_ss", "ss1"),
            cookie("web_id", "w1"),
        ];
        // 顺序无关：会话标记排序后哈希，别的 cookie 不参与
        let b = vec![
            cookie("sessionid_ss", "ss1"),
            cookie("sessionid", "s1"),
            cookie("web_id", "w2"),
        ];
        assert_eq!(session_fingerprint(&a), session_fingerprint(&b));
    }

    #[test]
    fn no_session_marker_means_not_logged_in() {
        assert!(!has_session(&[cookie("web_id", "w1")]));
        assert!(!has_session(&[cookie("sessionid", "")]));
    }

    #[test]
    fn parse_identity_prefers_lossless_string() {
        let data = json!({
            "user_id_str": "7000000000000000001",
            "passport_enterprise_user_type": "0",
            "name": "测试"
        });
        assert_eq!(
            parse_identity(&data),
            Some(("7000000000000000001".into(), "0".into(), "测试".into()))
        );
    }

    #[test]
    fn parse_identity_falls_back_to_numeric_only_when_lossless() {
        // user_id 落在 i64 内才敢用；大数会被 serde_json 压成 f64，不精确就不用
        let ok = json!({ "user_id": 123456, "name": "n" });
        assert_eq!(
            parse_identity(&ok).map(|(u, _, _)| u),
            Some("123456".into())
        );

        let missing = json!({ "name": "n" });
        assert!(parse_identity(&missing).is_none());
    }

    #[test]
    fn account_id_is_stable_per_identity() {
        assert_eq!(account_id_of("u1", "0"), account_id_of("u1", "0"));
        assert_ne!(account_id_of("u1", "0"), account_id_of("u1", "1"));
        assert_ne!(account_id_of("u1", "0"), account_id_of("u2", "0"));
    }
}
