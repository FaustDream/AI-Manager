//! 网络与档案自检：把 tools/intl-doctor.ps1 的三段探测搬进应用，设置页一键运行。
//!
//! 全部只读：端点探测不带任何凭据，档案检查只看文件是否存在。
//! 判定口径与 docs/rules/42 一致——「代理没开/节点挂了」是环境问题，
//! 自检的职责是让人一眼分清「该不该怪软件」。

use crate::app::net::Http;
use serde::Serialize;
use std::path::Path;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct EndpointCheck {
    pub url: String,
    pub ok: bool,
    /// HTTP 状态码，或失败原因（超时/连不上）
    pub detail: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct ProfileCheck {
    /// `cn/<uid>` 或 `intl/<uid>.tmp`
    pub label: String,
    /// 正式档案同时有 `app/session` 与 `auth.info`
    pub complete: bool,
    /// 只有 `<uid>.tmp`：旧版换名残留，下次切换自动认领，不是坏档
    pub tmp_only: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct SelfcheckReport {
    /// WinINET 系统代理是否启用（应用只认这一套 + 环境变量）
    pub proxy_enabled: bool,
    pub proxy_server: Option<String>,
    pub endpoints: Vec<EndpointCheck>,
    pub profiles: Vec<ProfileCheck>,
}

/// 三个国际端点：与 tools/intl-doctor.ps1 保持同一份清单。
const PROBE_URLS: [&str; 3] = [
    "https://www.workbuddy.ai/",
    "https://openapi.qoder.sh/api/v2/quota/usage",
    "https://qoder.com/",
];

/// 端点探测：走应用自己的 HTTP 层（含系统代理与超时配置）。
async fn probe_endpoints(http: &dyn Http) -> Vec<EndpointCheck> {
    let mut out = Vec::new();
    for url in PROBE_URLS {
        let check = match http.get(url).await {
            Ok(resp) => EndpointCheck {
                url: url.into(),
                // 401/403 也算「通」：能拿到 HTTP 响应就证明链路 + DNS + TLS 都好
                ok: resp.status < 500,
                detail: format!("HTTP {}", resp.status),
            },
            Err(e) => EndpointCheck {
                url: url.into(),
                ok: false,
                detail: e.message,
            },
        };
        out.push(check);
    }
    out
}

/// 档案完整性：`profiles/{cn,intl}/` 下每个正式目录必须同时有
/// `app/session` 与 `auth.info`；只剩 `<uid>.tmp` 是待认领，不算坏。
fn check_profiles(data_dir: &Path) -> Vec<ProfileCheck> {
    fn pack_complete(dir: &Path) -> bool {
        dir.join("app/session").is_dir() && dir.join("auth.info").is_file()
    }
    let mut out = Vec::new();
    for edition in ["cn", "intl"] {
        let root = data_dir.join("profiles").join(edition);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let (label, tmp_only) = if let Some(uid) = name.strip_suffix(".tmp") {
                (format!("{edition}/{uid}"), true)
            } else {
                (format!("{edition}/{name}"), false)
            };
            out.push(ProfileCheck {
                label,
                complete: pack_complete(&path),
                tmp_only,
            });
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// 跑一次完整自检。`http` 注入以便测试；生产走 `RealHttp`（含系统代理）。
pub async fn run(http: &dyn Http, data_dir: &Path) -> SelfcheckReport {
    let (proxy_enabled, proxy_server) = crate::app::net::system_proxy_status();
    SelfcheckReport {
        proxy_enabled,
        proxy_server,
        endpoints: probe_endpoints(http).await,
        profiles: check_profiles(data_dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::net::MockHttp;

    #[test]
    fn profiles_classify_complete_tmp_and_skip_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("profiles");
        let good = root.join("cn").join("u1");
        std::fs::create_dir_all(good.join("app/session")).unwrap();
        std::fs::write(good.join("auth.info"), "{}").unwrap();
        let tmp = root.join("intl").join("u2.tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let broken = root.join("intl").join("u3");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(root.join("cn").join("loose-file"), b"x").unwrap();

        let out = check_profiles(dir.path());
        assert_eq!(out.len(), 3, "散文件不算档案");
        let good = out.iter().find(|c| c.label == "cn/u1").unwrap();
        assert!(good.complete && !good.tmp_only);
        let tmp = out.iter().find(|c| c.label == "intl/u2").unwrap();
        assert!(tmp.tmp_only && !tmp.complete, "只剩 .tmp 是待认领不是坏档");
        let broken = out.iter().find(|c| c.label == "intl/u3").unwrap();
        assert!(!broken.complete && !broken.tmp_only, "正式目录缺件是坏档");
    }

    #[tokio::test]
    async fn endpoint_5xx_is_down_but_401_is_reachable() {
        let http = MockHttp::new();
        http.on(PROBE_URLS[0], 200, "{}");
        http.on(PROBE_URLS[1], 401, r#"{"error":"auth"}"#);
        http.on(PROBE_URLS[2], 502, "");
        let out = probe_endpoints(&http).await;
        assert!(out[0].ok);
        assert!(out[1].ok, "401 = 链路通、鉴权失败，不该报网络问题");
        assert!(!out[2].ok);
        assert_eq!(out[2].detail, "HTTP 502");
    }
}
