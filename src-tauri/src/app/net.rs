//! 网络注入。契约 1/2/3/5 需要断言「调了几次接口」，不注入就测不了。
//!
//! `calls()` 是这里最关键的能力：契约 2「当日已签不重复调用」靠它断言。

use crate::app::error::AppError;
use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

#[async_trait]
pub trait Http: Send + Sync {
    async fn get(&self, url: &str) -> Result<HttpResponse, AppError>;
    async fn post(&self, url: &str, body: &str) -> Result<HttpResponse, AppError>;
}

/// 测试用网络层：按 URL 预先排好响应，并记录每次调用。
pub struct MockHttp {
    routes: Mutex<HashMap<String, VecDeque<(u16, String)>>>,
    calls: Mutex<Vec<String>>,
}

impl MockHttp {
    pub fn new() -> Self {
        Self { routes: Mutex::new(HashMap::new()), calls: Mutex::new(Vec::new()) }
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
        self.calls.lock().unwrap().iter().filter(|c| c.as_str() == url).count()
    }
}

#[async_trait]
impl Http for MockHttp {
    async fn get(&self, url: &str) -> Result<HttpResponse, AppError> {
        self.calls.lock().unwrap().push(url.to_string());
        respond(&self.routes, url)
    }

    async fn post(&self, url: &str, _body: &str) -> Result<HttpResponse, AppError> {
        self.calls.lock().unwrap().push(url.to_string());
        respond(&self.routes, url)
    }
}

fn respond(
    routes: &Mutex<HashMap<String, VecDeque<(u16, String)>>>,
    url: &str,
) -> Result<HttpResponse, AppError> {
    let mut guard = routes.lock().unwrap();
    let queue = guard
        .get_mut(url)
        .ok_or_else(|| AppError::new("provider", format!("MockHttp 未预设该 URL：{url}")))?;
    // 队列里保留最后一个，用完继续返回它，避免测试因调用次数多一次就失败
    let (status, body) = if queue.len() > 1 {
        queue.pop_front().unwrap()
    } else {
        queue.front().cloned().unwrap_or((200, "{}".to_string()))
    };
    Ok(HttpResponse { status, body })
}
