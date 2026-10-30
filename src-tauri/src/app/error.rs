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

// 有了这几个转换，业务代码里可以直接 `?`，不必到处写 map_err
impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::new("sqlite", e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::new("io", e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::new("invalid", e.to_string())
    }
}
impl From<chrono::ParseError> for AppError {
    fn from(e: chrono::ParseError) -> Self {
        AppError::new("invalid", e.to_string())
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
