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
    // —— 以下三种是「切换登录」专用的失败态（P2 §5.1）——
    // 切换写的是客户端自己的登录态文件，写坏的代价是「客户端跳登录页」，
    // 所以失败必须能区分「没动过」和「动了但回滚了」，界面才能给出可操作的话术。
    /// 档案与目标账号对不上：拿 A 的票据恢复 B 的登录态比不恢复更糟
    ProfileMismatch,
    /// 写回失败，已按内存里的原文回滚
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

impl ErrorKind {
    /// 日志里的稳定标识：与序列化到前端的名字一致（snake_case），
    /// 这样日志里搜 `client_running` 和界面 toast 的 kind 是同一个词。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::Sqlite => "sqlite",
            Self::Provider => "provider",
            Self::Crypto => "crypto",
            Self::Invalid => "invalid",
            Self::Conflict => "conflict",
            Self::ClientRunning => "client_running",
            Self::Cancelled => "cancelled",
            Self::ProfileMismatch => "profile_mismatch",
            Self::WriteFailed => "write_failed",
            Self::SessionStateMissing => "session_state_missing",
            Self::Unsupported => "unsupported",
        }
    }
}

impl AppError {
    /// 日志用的完整证据：类别 + 原文 + `detail`（detail 本来就只给日志看，
    /// 不写进日志等于白收集）。
    pub fn evidence(&self) -> String {
        match self.detail.as_deref().filter(|d| !d.is_empty()) {
            Some(detail) => format!(
                "kind={} · 原文={} · 详情={detail}",
                self.kind.as_str(),
                self.message
            ),
            None => format!("kind={} · 原文={}", self.kind.as_str(), self.message),
        }
    }
}

impl AppError {
    /// 唯一构造入口：kind 收枚举，编译期钉死（架构评审 C5）——
    /// 拼错类别在写代码时就红，不会再静默落 Provider 让前端话术跟着错。
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
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
        AppError::new(ErrorKind::Sqlite, e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::new(ErrorKind::Io, e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::new(ErrorKind::Invalid, e.to_string())
    }
}
impl From<chrono::ParseError> for AppError {
    fn from(e: chrono::ParseError) -> Self {
        AppError::new(ErrorKind::Invalid, e.to_string())
    }
}

impl From<windows::core::Error> for AppError {
    fn from(e: windows::core::Error) -> Self {
        AppError::new(ErrorKind::Io, format!("Windows API 调用失败：{e}"))
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
