//! 网络注入。契约 1/2/3/5 需要断言「调了几次接口」，不注入就测不了。
//!
//! `calls()` 是这里最关键的能力：契约 2「当日已签不重复调用」靠它断言。
//!
//! P3 起请求要带鉴权头与 JSON body，所以统一成 `HttpRequest`。
//! `RealHttp`（reqwest）只在这里出现一次，业务代码拿到的永远是 `&dyn Http`。

use crate::app::error::{AppError, ErrorKind, Result};
use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

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
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            // 各家客户端都会校验 UA；空 UA 会被部分网关直接拒绝
            .user_agent("AI-Manager")
            .build()
            // 构造失败只可能是 TLS 后端初始化问题；宁可启动期崩，
            // 也别退化成无超时的 client——签到请求会永久挂起占住配额
            .expect("reqwest client 构建失败");
        Self { client }
    }
}

impl Default for RealHttp {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Http for RealHttp {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse> {
        let mut builder = match req.method {
            Method::Get => self.client.get(&req.url),
            Method::Post => self.client.post(&req.url),
        };
        for (key, value) in &req.headers {
            builder = builder.header(key, value);
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let resp = builder.send().await.map_err(|e| {
            AppError::new(ErrorKind::Provider, format!("网络请求失败：{}", brief(&e)))
        })?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok(HttpResponse { status, body })
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
