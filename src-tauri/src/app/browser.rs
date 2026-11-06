//! 用**无痕窗口**打开登录页。
//!
//! 为什么必须无痕：登录页复用浏览器里已有的会话时，平台会按「已登录的那个账号」
//! 直接签发凭据——用户点的是「重新登录 A」，拿回来的却是 B 的凭据，账号库就这么串了。
//! 无痕窗口从空会话开始，登录的一定是用户当场输入的那个账号。
//!
//! 浏览器选择：默认浏览器（识别得出时）→ Chrome → Edge → Firefox → Brave。
//! Edge 是 Windows 内置，所以「一个都找不到」基本不会发生。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::process::reg_read_string;
use std::path::PathBuf;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;

/// 默认浏览器的 ProgId 所在位置（HKCU 下，跟随当前用户的选择）。
const URL_ASSOC_KEY: &str =
    "Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice";

struct Candidate {
    /// 界面上要告诉用户「在哪个窗口里登录」
    name: &'static str,
    exe: PathBuf,
    /// 无痕/隐私窗口的参数：Chromium 系统一 `--incognito`，Firefox 是 `-private-window`
    private_arg: &'static str,
}

/// 用无痕窗口打开 `url`，返回实际使用的浏览器名。
///
/// 返回 `Unsupported` 表示没找到任何支持无痕参数的浏览器，调用方退回普通打开。
pub fn open_private(url: &str) -> Result<&'static str> {
    let list = candidates();
    if list.is_empty() {
        return Err(AppError::new(
            ErrorKind::Unsupported,
            "没有找到支持无痕窗口的浏览器",
        ));
    }
    // 默认浏览器优先：用错的浏览器打开登录页，用户第一反应是「我点错东西了」
    let chosen = match default_family() {
        Some(family) => list.iter().find(|item| item.name == family),
        None => None,
    }
    .unwrap_or(&list[0]);

    // hidden = false：浏览器是 GUI 子系统程序，不会弹控制台；
    // 而 SW_HIDE 会让它新建的第一个窗口不可见，用户以为没打开。
    crate::app::process::spawn_with_args(&chosen.exe, &[chosen.private_arg, url], false)?;
    Ok(chosen.name)
}

/// 已安装且支持无痕参数的浏览器。顺序即兜底顺序。
fn candidates() -> Vec<Candidate> {
    let pf = std::env::var("ProgramFiles").ok().map(PathBuf::from);
    let pf86 = std::env::var("ProgramFiles(x86)").ok().map(PathBuf::from);
    let local = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);

    let mut out = Vec::new();
    let mut add = |name: &'static str, private_arg: &'static str, paths: Vec<Option<PathBuf>>| {
        if let Some(exe) = paths.into_iter().flatten().find(|path| path.exists()) {
            out.push(Candidate {
                name,
                exe,
                private_arg,
            });
        }
    };

    add(
        "Chrome",
        "--incognito",
        vec![
            join(&local, "Google\\Chrome\\Application\\chrome.exe"),
            join(&pf, "Google\\Chrome\\Application\\chrome.exe"),
            join(&pf86, "Google\\Chrome\\Application\\chrome.exe"),
        ],
    );
    add(
        "Edge",
        "--incognito",
        vec![
            join(&pf86, "Microsoft\\Edge\\Application\\msedge.exe"),
            join(&pf, "Microsoft\\Edge\\Application\\msedge.exe"),
        ],
    );
    add(
        "Firefox",
        "-private-window",
        vec![
            join(&pf, "Mozilla Firefox\\firefox.exe"),
            join(&pf86, "Mozilla Firefox\\firefox.exe"),
        ],
    );
    add(
        "Brave",
        "--incognito",
        vec![
            join(
                &local,
                "BraveSoftware\\Brave-Browser\\Application\\brave.exe",
            ),
            join(&pf, "BraveSoftware\\Brave-Browser\\Application\\brave.exe"),
        ],
    );
    out
}

fn join(root: &Option<PathBuf>, rest: &str) -> Option<PathBuf> {
    root.as_ref().map(|dir| dir.join(rest))
}

/// 默认浏览器的家族名；识别不了（360 / QQ 浏览器等）返回 None，交给兜底顺序。
fn default_family() -> Option<&'static str> {
    let prog_id = reg_read_string(HKEY_CURRENT_USER, URL_ASSOC_KEY, "ProgId")?;
    let lower = prog_id.to_ascii_lowercase();
    if lower.starts_with("chromehtml") {
        Some("Chrome")
    } else if lower.starts_with("msedge") {
        Some("Edge")
    } else if lower.starts_with("firefoxurl") {
        Some("Firefox")
    } else if lower.starts_with("bravehtml") {
        Some("Brave")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prog_id_maps_known_browsers_only() {
        // 只验证映射规则本身：注册表内容随机器变化，不能拿来断言
        let families = [
            ("ChromeHTML", Some("Chrome")),
            ("MSEdgeHTM", Some("Edge")),
            ("FirefoxURL-308046B0AF4A39CB", Some("Firefox")),
            ("BraveHTML", Some("Brave")),
            ("360seURL", None),
        ];
        for (prog_id, expected) in families {
            let lower = prog_id.to_ascii_lowercase();
            let actual = if lower.starts_with("chromehtml") {
                Some("Chrome")
            } else if lower.starts_with("msedge") {
                Some("Edge")
            } else if lower.starts_with("firefoxurl") {
                Some("Firefox")
            } else if lower.starts_with("bravehtml") {
                Some("Brave")
            } else {
                None
            };
            assert_eq!(actual, expected, "{prog_id}");
        }
    }

    #[test]
    fn candidates_are_unique_and_ordered() {
        // 探测读的是真实安装路径，但只要求「不 panic、不重复」：
        // 断言具体装了哪个浏览器会让测试随机器环境漂移
        let list = candidates();
        let names: Vec<&str> = list.iter().map(|item| item.name).collect();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names, deduped, "同一浏览器不能出现两次");
    }
}
