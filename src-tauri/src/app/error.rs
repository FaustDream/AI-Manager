//! 统一错误类型。前端按 kind 决定提示文案，各页面不自己拼错误消息。

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Io,
    Sqlite,
    Provider,
    Crypto,
    Invalid,
    /// 平台里存在同名但非本工具写入的条目（契约 16）
    Conflict,
    /// 目标客户端正在运行，不能写（契约 9）
    ClientRunning,
    /// 用户主动取消——不能被记成失败
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    /// 只用于日志，不展示到界面
    pub detail: Option<String>,
}

impl AppError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: match kind {
                "io" => ErrorKind::Io,
                "sqlite" => ErrorKind::Sqlite,
                "crypto" => ErrorKind::Crypto,
                "invalid" => ErrorKind::Invalid,
                "conflict" => ErrorKind::Conflict,
                "client_running" => ErrorKind::ClientRunning,
                "cancelled" => ErrorKind::Cancelled,
                _ => ErrorKind::Provider,
            },
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn is_cancelled(&self) -> bool {
        self.kind == ErrorKind::Cancelled
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
