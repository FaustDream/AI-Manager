//! 七个技能 / MCP 目标的路径表。
//!
//! **这是唯一的事实来源**：技能部署、MCP 写入、导入扫描都从这里取，
//! 任何地方都不准再写 `if platform == "qoder"` 这种分支（写岔一次就是静默失效）。
//!
//! 国内版普遍带 `-cn`，WorkBuddy 是例外：国内 `~/.workbuddy`、国际 `~/.workbuddy-ai`。

use crate::contracts::TargetKey;
use crate::types::PlatformKey;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStyle {
    /// 标准 `mcpServers`，停用用 `disabled: true`
    Standard,
    /// Trae：stdio 条目习惯带 `url` / `headers` 空值
    Trae,
    /// CodeBuddy：条目必须带 `disabled` 字段
    CodeBuddy,
    /// ZCode：`mcp.servers`，停用用 `enabled: false`，条目带 `type` / `timeoutMs`
    ZCode,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub key: TargetKey,
    pub label: &'static str,
    /// 技能目录（相对家目录的段）
    pub skill_dir: &'static [&'static str],
    /// MCP 配置文件（相对根目录的段）
    pub mcp_file: &'static [&'static str],
    /// MCP 配置文件在 `%APPDATA%` 下而不是家目录下
    pub appdata: bool,
    pub style: McpStyle,
    /// 界面上的备注，解释为什么是这个路径
    pub note: &'static str,
}

/// 一张表，一条一行。加平台 = 加一行，逻辑不动。
pub const TARGETS: &[Target] = &[
    Target {
        key: TargetKey::QoderCn,
        label: "Qoder CN",
        skill_dir: &[".qoder-cn", "skills"],
        mcp_file: &[".qoder-cn", "settings.json"],
        appdata: false,
        style: McpStyle::Standard,
        note: "国内版带 -cn",
    },
    Target {
        key: TargetKey::Qoder,
        label: "Qoder 国际版",
        skill_dir: &[".qoder", "skills"],
        mcp_file: &[".qoder", "settings.json"],
        appdata: false,
        style: McpStyle::Standard,
        note: "",
    },
    Target {
        key: TargetKey::TraeCn,
        label: "Trae CN",
        skill_dir: &[".trae-cn", "skills"],
        mcp_file: &["Trae CN", "User", "mcp.json"],
        appdata: true,
        style: McpStyle::Trae,
        note: "MCP 配置在 %APPDATA%",
    },
    Target {
        key: TargetKey::Trae,
        label: "Trae 国际版",
        skill_dir: &[".trae", "skills"],
        mcp_file: &["Trae", "User", "mcp.json"],
        appdata: true,
        style: McpStyle::Trae,
        note: "MCP 配置在 %APPDATA%",
    },
    Target {
        key: TargetKey::WorkBuddy,
        label: "WorkBuddy 国内版",
        skill_dir: &[".workbuddy", "skills"],
        mcp_file: &[".workbuddy", "mcp.json"],
        appdata: false,
        style: McpStyle::Standard,
        note: "例外：国内版不带 -cn",
    },
    Target {
        key: TargetKey::WorkBuddyIntl,
        label: "WorkBuddy 国际版",
        skill_dir: &[".workbuddy-ai", "skills"],
        mcp_file: &[".workbuddy-ai", "mcp.json"],
        appdata: false,
        style: McpStyle::Standard,
        note: "例外：不是 -cn 规则",
    },
    Target {
        key: TargetKey::CodeBuddy,
        label: "CodeBuddy（IDE / 插件）",
        skill_dir: &[".codebuddy", "skills"],
        mcp_file: &[".codebuddy", "mcp.json"],
        appdata: false,
        style: McpStyle::CodeBuddy,
        note: "IDE 与插件版共用此目录",
    },
    Target {
        key: TargetKey::ZCode,
        label: "ZCode",
        skill_dir: &[".zcode", "skills"],
        mcp_file: &[".zcode", "cli", "config.json"],
        appdata: false,
        style: McpStyle::ZCode,
        note: "MCP 在 mcp.servers 下，停用用 enabled:false",
    },
];

pub fn get(key: TargetKey) -> &'static Target {
    TARGETS
        .iter()
        .find(|target| target.key == key)
        .expect("每个 TargetKey 都必须在表里")
}

pub fn label(key: TargetKey) -> &'static str {
    get(key).label
}

/// 稳定的字符串键：文件名、界面取值、日志都用它。
pub fn key_name(key: TargetKey) -> &'static str {
    match key {
        TargetKey::QoderCn => "qoder_cn",
        TargetKey::Qoder => "qoder_intl",
        TargetKey::TraeCn => "trae_cn",
        TargetKey::Trae => "trae_intl",
        TargetKey::WorkBuddy => "workbuddy_cn",
        TargetKey::WorkBuddyIntl => "workbuddy_intl",
        TargetKey::CodeBuddy => "codebuddy",
        TargetKey::ZCode => "zcode",
    }
}

/// 技能目录（`~/.qoder-cn/skills`）。
pub fn skill_dir(home: &Path, key: TargetKey) -> PathBuf {
    get(key)
        .skill_dir
        .iter()
        .fold(home.to_path_buf(), |path, part| path.join(part))
}

/// MCP 配置文件（Trae 在 `%APPDATA%` 下，其余在家目录）。
pub fn mcp_file(home: &Path, appdata: &Path, key: TargetKey) -> PathBuf {
    let target = get(key);
    // Trae 的配置在 %APPDATA%\Trae CN\User\mcp.json，其余都在家目录下
    let base = if target.appdata { appdata } else { home };
    target
        .mcp_file
        .iter()
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

/// 字符串键反查（界面传上来的是 `qoder_cn` 这种）。
pub fn from_key_name(name: &str) -> Option<TargetKey> {
    TARGETS
        .iter()
        .map(|t| t.key)
        .find(|key| key_name(*key) == name)
}

/// 账号平台键 → 技能目标键，用于「这个账号对应哪个技能目录」。
pub fn from_platform(platform: PlatformKey) -> TargetKey {
    match platform {
        PlatformKey::TraeCn => TargetKey::TraeCn,
        PlatformKey::Trae => TargetKey::Trae,
        PlatformKey::QoderCn => TargetKey::QoderCn,
        PlatformKey::QoderIntl => TargetKey::Qoder,
        PlatformKey::WorkBuddy => TargetKey::WorkBuddy,
        PlatformKey::WorkBuddyIntl => TargetKey::WorkBuddyIntl,
        PlatformKey::ZCode => TargetKey::ZCode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_in_the_table_once() {
        let keys = [
            TargetKey::QoderCn,
            TargetKey::Qoder,
            TargetKey::TraeCn,
            TargetKey::Trae,
            TargetKey::WorkBuddy,
            TargetKey::WorkBuddyIntl,
            TargetKey::CodeBuddy,
            TargetKey::ZCode,
        ];
        for key in keys {
            assert_eq!(
                TARGETS.iter().filter(|t| t.key == key).count(),
                1,
                "{key:?} 缺失或重复"
            );
        }
    }

    #[test]
    fn domestic_versions_carry_the_cn_suffix_except_workbuddy() {
        let home = Path::new("C:/Users/x");
        let appdata = Path::new("C:/Users/x/AppData/Roaming");
        assert!(skill_dir(home, TargetKey::QoderCn).ends_with(".qoder-cn/skills"));
        assert!(skill_dir(home, TargetKey::TraeCn).ends_with(".trae-cn/skills"));
        assert!(skill_dir(home, TargetKey::WorkBuddy).ends_with(".workbuddy/skills"));
        assert!(skill_dir(home, TargetKey::WorkBuddyIntl).ends_with(".workbuddy-ai/skills"));
        assert!(mcp_file(home, appdata, TargetKey::TraeCn).ends_with("Trae CN/User/mcp.json"));
        assert!(mcp_file(home, appdata, TargetKey::ZCode).ends_with(".zcode/cli/config.json"));
    }
}
