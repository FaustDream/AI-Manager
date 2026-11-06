//! 版本检测：只查 GitHub Releases，不自动下载、不自动替换。
//!
//! 桌面工具的更新必须是用户点的：自动替换 exe 会被杀软拦、也会把正在跑的
//! 任务打断。这里只回答「有没有新版本、发在哪」。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use serde::Serialize;

const LATEST_URL: &str = "https://api.github.com/repos/FaustDream/AI-Manager/releases/latest";

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub has_update: bool,
    /// 发布页地址；没有新版本时也给，方便手动看更新说明
    pub url: String,
    pub published_at: Option<String>,
    /// 发布说明（截断到 4000 字，避免把整个 CHANGELOG 塞进界面）
    pub notes: String,
}

/// 语义化版本比较：只比 `主.次.修订`，预发布后缀（`-beta.1`）按「不大于正式版」处理。
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

pub async fn check(http: &dyn Http, current: &str) -> Result<UpdateInfo> {
    let request = HttpRequest::get(LATEST_URL)
        // GitHub API 要求带 UA，否则直接 403
        .header("User-Agent", "AI-Manager")
        .header("Accept", "application/vnd.github+json");
    let response = http.send(request).await?;
    if response.status == 403 {
        return Err(AppError::new(
            ErrorKind::RateLimited,
            "GitHub 接口限流（403），过一会儿再试",
        ));
    }
    if !response.is_ok() {
        return Err(AppError::new(
            ErrorKind::Provider,
            format!("检查更新失败：HTTP {}", response.status),
        ));
    }
    let body = response.json()?;
    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if tag.is_empty() {
        return Err(AppError::new(
            ErrorKind::Provider,
            "GitHub 没有返回版本号（可能还没有正式 Release）",
        ));
    }
    let latest = tag.trim_start_matches(['v', 'V']).to_string();
    let mut notes = body
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if notes.chars().count() > 4000 {
        notes = notes.chars().take(4000).collect::<String>() + "…";
    }
    Ok(UpdateInfo {
        current: current.to_string(),
        has_update: is_newer(&latest, current),
        latest,
        url: body
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("https://github.com/FaustDream/AI-Manager/releases")
            .to_string(),
        published_at: body
            .get("published_at")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::net::MockHttp;

    #[test]
    fn version_compare_handles_prefix_and_suffix() {
        assert!(is_newer("1.9.0", "1.8.2"));
        assert!(is_newer("v1.10.0", "1.9.9"));
        assert!(!is_newer("1.8.2", "1.8.2"));
        assert!(!is_newer("1.8.1", "1.8.2"));
        assert!(is_newer("1.9.0-beta.1", "1.8.2"), "预发布也按数值比较");
    }

    #[tokio::test]
    async fn check_parses_release_and_flags_update() {
        let http = MockHttp::new();
        http.on(
            LATEST_URL,
            200,
            r#"{"tag_name":"v1.9.0","html_url":"https://example.com/r","published_at":"2026-11-01T00:00:00Z","body":"更新内容"}"#,
        );
        let info = check(&http, "1.8.2").await.unwrap();
        assert_eq!(info.latest, "1.9.0");
        assert!(info.has_update);
        assert_eq!(info.url, "https://example.com/r");
        assert_eq!(info.notes, "更新内容");
    }

    #[tokio::test]
    async fn rate_limit_is_reported_as_such() {
        let http = MockHttp::new();
        http.on(LATEST_URL, 403, "{}");
        let err = check(&http, "1.8.2").await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::RateLimited);
    }
}
