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
    // —— 以下四种是「切换登录」专用的失败态（P2 §5.1）——
    // 切换写的是客户端自己的登录态文件，写坏的代价是「客户端跳登录页」，
    // 所以失败必须能区分「没动过」和「动了但回滚了」，界面才能给出可操作的话术。
    /// 写前备份失败：此时**一个字节都没写**，取消即可
    BackupFailed,
    /// 档案与目标账号对不上：拿 A 的票据恢复 B 的登录态比不恢复更糟
    ProfileMismatch,
    /// 写回失败，已按备份回滚
    WriteFailed,
    /// 票据里缺 `sessionState` 之类的服务端签发字段，写回去会停在登录页
    SessionStateMissing,
    /// 该平台的登录态无法写回（Trae 是设备绑定加密）
    Unsupported,
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
                "backup_failed" => ErrorKind::BackupFailed,
                "profile_mismatch" => ErrorKind::ProfileMismatch,
                "write_failed" => ErrorKind::WriteFailed,
                "session_state_missing" => ErrorKind::SessionStateMissing,
                "unsupported" => ErrorKind::Unsupported,
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

impl From<windows::core::Error> for AppError {
    fn from(e: windows::core::Error) -> Self {
        AppError::new("io", format!("Windows API 调用失败：{e}"))
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
