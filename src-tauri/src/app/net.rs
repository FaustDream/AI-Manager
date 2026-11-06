//! 网络注入。契约 1/2/3/5 需要断言「调了几次接口」，不注入就测不了。
//!
//! `calls()` 是这里最关键的能力：契约 2「当日已签不重复调用」靠它断言。
//!
//! P3 起请求要带鉴权头与 JSON body，所以统一成 `HttpRequest`。
//! `RealHttp`（reqwest）只在这里出现一次，业务代码拿到的永远是 `&dyn Http`。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::{log, process};
use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// 解析成 JSON；解析失败给的是「返回内容不是 JSON」而不是裸 panic。
    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_str(&self.body).map_err(|_| {
            AppError::new(
                ErrorKind::Provider,
                format!("HTTP {}（返回内容不是 JSON）", self.status),
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// 一次请求。`url` 含查询串（MockHttp 就按完整 url 记调用）。
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn post(url: impl Into<String>) -> Self {
        Self {
            method: Method::Post,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn header(mut self, key: &str, value: impl Into<String>) -> Self {
        self.headers.push((key.to_string(), value.into()));
        self
    }

    pub fn headers(mut self, pairs: &[(String, String)]) -> Self {
        self.headers.extend_from_slice(pairs);
        self
    }

    pub fn json_body(mut self, value: &serde_json::Value) -> Self {
        self.body = Some(value.to_string());
        self.header("Content-Type", "application/json")
    }
}

#[async_trait]
pub trait Http: Send + Sync {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse>;

    async fn get(&self, url: &str) -> Result<HttpResponse> {
        self.send(HttpRequest::get(url)).await
    }

    async fn post(&self, url: &str, body: &str) -> Result<HttpResponse> {
        let mut req = HttpRequest::post(url);
        if !body.is_empty() {
            req.body = Some(body.to_string());
            req.headers
                .push(("Content-Type".into(), "application/json".into()));
        }
        self.send(req).await
    }
}

/// 真实网络。整个进程共用一个 client（连接复用；每请求超时 15s）。
pub struct RealHttp {
    client: reqwest::Client,
}

impl RealHttp {
    pub fn new() -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            // 各家客户端都会校验 UA；空 UA 会被部分网关直接拒绝
            .user_agent("AI-Manager");
        if let Some((url, bypass)) = system_proxy() {
            match reqwest::Proxy::all(&url) {
                Ok(proxy) => {
                    // 排除列表挂在代理上（ClientBuilder::no_proxy 是无参的「彻底不用代理」）
                    let skip = bypass_list(&bypass);
                    let proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&skip));
                    builder = builder.proxy(proxy);
                    log::log(log::Level::Info, "net", "已启用系统代理（Internet 选项）");
                }
                // 代理地址写错不该让整个应用没网：退回直连，并在日志里说清楚
                Err(e) => log::log(
                    log::Level::Warn,
                    "net",
                    format!("系统代理地址不可用，已退回直连：{e}"),
                ),
            }
        }
        let client = builder
            .build()
            // 构造失败只可能是 TLS 后端初始化问题；宁可启动期崩，
            // 也别退化成无超时的 client——签到请求会永久挂起占住配额
            .expect("reqwest client 构建失败");
        Self { client }
    }
}

/// Windows 系统代理（「Internet 选项」里的那套）。
///
/// reqwest 只认 `HTTP_PROXY` / `HTTPS_PROXY` 环境变量，**不读** WinINET 的系统代理。
/// 而绝大多数用户只把代理配在系统里：浏览器与各家客户端都走代理，唯独本应用直连 ——
/// 表现就是国际版 WorkBuddy / Qoder 的登录与查询全部「请求超时」，而国内域名一切正常。
///
/// 返回 `(proxy url, bypass 文本)`；环境变量已存在时返回 None，交给 reqwest 自己处理，
/// 免得两套配置互相覆盖。
fn system_proxy() -> Option<(String, String)> {
    if ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
    {
        return None;
    }
    const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
    // ProxyEnable 为 0 时 ProxyServer 常常还留着上次的残值，不能只看字符串非空
    if process::reg_read_dword(HKEY_CURRENT_USER, KEY, "ProxyEnable")? == 0 {
        return None;
    }
    let url = proxy_url(&process::reg_read_string(
        HKEY_CURRENT_USER,
        KEY,
        "ProxyServer",
    )?)?;
    let bypass =
        process::reg_read_string(HKEY_CURRENT_USER, KEY, "ProxyOverride").unwrap_or_default();
    Some((url, bypass))
}

/// `ProxyServer` 有两种写法：`host:port`，或 `http=host:port;https=host:port`。
/// 后者的 https 项优先——本应用的请求几乎全是 https。
fn proxy_url(server: &str) -> Option<String> {
    let text = server.trim();
    if text.is_empty() {
        return None;
    }
    let picked = text
        .split(';')
        .filter_map(|item| item.split_once('='))
        .find(|(scheme, _)| scheme.trim().eq_ignore_ascii_case("https"))
        .map(|(_, value)| value.trim())
        .unwrap_or_else(|| text.split(';').next().unwrap_or(text).trim());
    if picked.is_empty() {
        return None;
    }
    Some(if picked.contains("://") {
        picked.to_string()
    } else {
        format!("http://{picked}")
    })
}

/// `ProxyOverride` 用 `;` 分隔，还带 `<local>` / `<-loopback>` 这类伪项；
/// reqwest 的 no_proxy 只认域名列表。
fn bypass_list(override_text: &str) -> String {
    override_text
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty() && !item.starts_with('<'))
        .collect::<Vec<_>>()
        .join(",")
}

impl Default for RealHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl RealHttp {
    /// 真正发一次请求。错误带上「是否值得重试」，由 `send` 决定。
    async fn once(&self, req: &HttpRequest) -> std::result::Result<HttpResponse, (AppError, bool)> {
        let mut builder = match req.method {
            Method::Get => self.client.get(&req.url),
            Method::Post => self.client.post(&req.url),
        };
        for (key, value) in &req.headers {
            builder = builder.header(key, value);
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }
        let resp = builder.send().await.map_err(|e| {
            // 超时/连不上属于链路问题，重试有意义；解码失败之类重试也没用
            let retry = e.is_timeout() || e.is_connect();
            (
                AppError::new(ErrorKind::Provider, format!("网络请求失败：{}", brief(&e))),
                retry,
            )
        })?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok(HttpResponse { status, body })
    }
}

#[async_trait]
impl Http for RealHttp {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        match self.once(&req).await {
            Ok(resp) => Ok(resp),
            // GET 是只读的：跨境链路 + 本地代理偶发重置时重试一次，别让账号列表
            // 因为一次抖动就挂上「查询失败」的红字。
            // POST 一律不重试——签到接口重放等于重复领取。
            Err((e, retry)) if retry && req.method == Method::Get => {
                log::log(
                    log::Level::Debug,
                    "net",
                    format!("GET 失败，重试一次：{}", e.message),
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
                self.once(&req).await.map_err(|(e, _)| e)
            }
            Err((e, _)) => Err(e),
        }
    }
}

/// reqwest 的错误 Display 会带上完整 URL（里面有查询参数），日志里只留一层原因。
fn brief(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "请求超时".to_string()
    } else if e.is_connect() {
        "连不上服务器（网络或代理）".to_string()
    } else if e.is_decode() {
        "响应解码失败".to_string()
    } else {
        e.to_string()
    }
}

/// 测试用网络层：按 URL 预先排好响应，并记录每次调用。
pub struct MockHttp {
    routes: Mutex<HashMap<String, VecDeque<(u16, String)>>>,
    calls: Mutex<Vec<String>>,
    /// 记录每次请求的请求头，供「鉴权头有没有带」这类断言使用
    // type_complexity：测试夹具的私有字段，抽 type 出来反而是噪音
    #[allow(clippy::type_complexity)]
    headers: Mutex<Vec<(String, Vec<(String, String)>)>>,
}

impl MockHttp {
    pub fn new() -> Self {
        Self {
            routes: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            headers: Mutex::new(Vec::new()),
        }
    }

    /// 同一个 URL 每次都返回这个响应。
    pub fn on(&self, url: &str, status: u16, body: &str) {
        self.routes
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_default()
            .push_back((status, body.to_string()));
    }

    /// 依次返回不同响应，用完后重复最后一个（模拟「第一次未签、第二次已签」）。
    pub fn on_sequence(&self, url: &str, items: Vec<(u16, &str)>) {
        // 必须先用 let 持有 guard，否则锁的临时值在这条语句结束就被释放
        let mut guard = self.routes.lock().unwrap();
        let queue = guard.entry(url.to_string()).or_default();
        for (status, body) in items {
            queue.push_back((status, body.to_string()));
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call_count(&self, url: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.as_str() == url)
            .count()
    }

    /// 某个 URL 最近一次请求带的头（大小写不敏感查找）。
    pub fn last_header(&self, url: &str, key: &str) -> Option<String> {
        let guard = self.headers.lock().unwrap();
        guard
            .iter()
            .rev()
            .find(|(u, _)| u == url)
            .and_then(|(_, hs)| {
                hs.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v.clone())
            })
    }
}

impl Default for MockHttp {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Http for MockHttp {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.calls.lock().unwrap().push(req.url.clone());
        self.headers
            .lock()
            .unwrap()
            .push((req.url.clone(), req.headers.clone()));
        respond(&self.routes, &req.url)
    }
}

fn respond(
    routes: &Mutex<HashMap<String, VecDeque<(u16, String)>>>,
    url: &str,
) -> Result<HttpResponse> {
    let mut guard = routes.lock().unwrap();
    let queue = guard.get_mut(url).ok_or_else(|| {
        AppError::new(ErrorKind::Provider, format!("MockHttp 未预设该 URL：{url}"))
    })?;
    // 队列里保留最后一个，用完继续返回它，避免测试因调用次数多一次就失败
    let (status, body) = if queue.len() > 1 {
        queue.pop_front().unwrap()
    } else {
        queue.front().cloned().unwrap_or((200, "{}".to_string()))
    };
    Ok(HttpResponse { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_server_accepts_both_windows_shapes() {
        assert_eq!(
            proxy_url("127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        // 分段写法优先取 https
        assert_eq!(
            proxy_url("http=127.0.0.1:7890;https=127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        // 已经带 scheme 就原样用
        assert_eq!(
            proxy_url("socks5://127.0.0.1:7897").as_deref(),
            Some("socks5://127.0.0.1:7897")
        );
        assert_eq!(proxy_url("   ").as_deref(), None);
    }

    #[test]
    fn proxy_bypass_drops_windows_pseudo_entries() {
        assert_eq!(
            bypass_list("<local>;*.corp.example.com;10.*"),
            "*.corp.example.com,10.*"
        );
        assert_eq!(bypass_list("<-loopback>"), "");
        assert_eq!(bypass_list(""), "");
    }

    #[tokio::test]
    async fn mock_records_calls_and_headers() {
        let http = MockHttp::new();
        http.on("https://x/a", 200, r#"{"ok":1}"#);
        let req = HttpRequest::get("https://x/a").header("Authorization", "Bearer t");
        let resp = http.send(req).await.unwrap();
        assert_eq!(resp.json().unwrap()["ok"], 1);
        assert_eq!(http.call_count("https://x/a"), 1);
        assert_eq!(
            http.last_header("https://x/a", "authorization").as_deref(),
            Some("Bearer t")
        );
    }

    #[tokio::test]
    async fn sequence_repeats_last_response() {
        let http = MockHttp::new();
        http.on_sequence(
            "https://x/a",
            vec![(200, r#"{"n":1}"#), (200, r#"{"n":2}"#)],
        );
        assert_eq!(
            http.get("https://x/a").await.unwrap().json().unwrap()["n"],
            1
        );
        assert_eq!(
            http.get("https://x/a").await.unwrap().json().unwrap()["n"],
            2
        );
        assert_eq!(
            http.get("https://x/a").await.unwrap().json().unwrap()["n"],
            2,
            "用完队列后应重复最后一个响应"
        );
    }

    #[test]
    fn non_json_body_is_a_provider_error() {
        let resp = HttpResponse {
            status: 200,
            body: "<html>".into(),
        };
        assert_eq!(
            resp.json().unwrap_err().kind,
            crate::app::error::ErrorKind::Provider
        );
    }
}
