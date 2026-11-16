//! 八个技能 / MCP 目标的路径表。
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

/// 路径的根。三家平台把东西放在三个不同的地方，写死成「家目录」一定会错。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    Home,
    /// `%APPDATA%`（Roaming）
    Appdata,
    /// `%LOCALAPPDATA%`
    LocalData,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub key: TargetKey,
    pub label: &'static str,
    /// 技能目录的根
    pub skill_root: Root,
    /// 技能目录（相对根的段）
    pub skill_dir: &'static [&'static str],
    /// MCP 配置的根
    pub mcp_root: Root,
    /// MCP 配置文件（相对根的段）；**空数组 = 这个平台没有可写的本地 MCP 配置**
    pub mcp_file: &'static [&'static str],
    pub style: McpStyle,
    /// 界面上的备注，解释为什么是这个路径
    pub note: &'static str,
}

impl Target {
    /// 这个平台有没有可写的 MCP 配置文件（豆包这类只在应用内管理 MCP）。
    pub fn supports_mcp(&self) -> bool {
        !self.mcp_file.is_empty()
    }
}

/// 一张表，一条一行。加平台 = 加一行，逻辑不动。
pub const TARGETS: &[Target] = &[
    Target {
        key: TargetKey::QoderCn,
        label: "Qoder CN",
        skill_dir: &[".qoder-cn", "skills"],
        mcp_file: &[".qoder-cn", "settings.json"],
        skill_root: Root::Home,
        mcp_root: Root::Home,
        style: McpStyle::Standard,
        note: "国内版带 -cn",
    },
    Target {
        key: TargetKey::Qoder,
        label: "Qoder 国际版",
        skill_dir: &[".qoder", "skills"],
        mcp_file: &[".qoder", "settings.json"],
        skill_root: Root::Home,
        mcp_root: Root::Home,
        style: McpStyle::Standard,
        note: "",
    },
    Target {
        key: TargetKey::TraeCn,
        label: "Trae CN",
        skill_dir: &[".trae-cn", "skills"],
        mcp_file: &["Trae CN", "User", "mcp.json"],
        skill_root: Root::Home,
        mcp_root: Root::Appdata,
        style: McpStyle::Trae,
        note: "MCP 配置在 %APPDATA%",
    },
    Target {
        key: TargetKey::Trae,
        label: "Trae 国际版",
        skill_dir: &[".trae", "skills"],
        mcp_file: &["Trae", "User", "mcp.json"],
        skill_root: Root::Home,
        mcp_root: Root::Appdata,
        style: McpStyle::Trae,
        note: "MCP 配置在 %APPDATA%",
    },
    Target {
        key: TargetKey::WorkBuddy,
        label: "WorkBuddy 国内版",
        skill_dir: &[".workbuddy", "skills"],
        mcp_file: &[".workbuddy", "mcp.json"],
        skill_root: Root::Home,
        mcp_root: Root::Home,
        style: McpStyle::Standard,
        note: "例外：国内版不带 -cn",
    },
    Target {
        key: TargetKey::WorkBuddyIntl,
        label: "WorkBuddy 国际版",
        skill_dir: &[".workbuddy-ai", "skills"],
        mcp_file: &[".workbuddy-ai", "mcp.json"],
        skill_root: Root::Home,
        mcp_root: Root::Home,
        style: McpStyle::Standard,
        note: "例外：不是 -cn 规则",
    },
    Target {
        key: TargetKey::CodeBuddy,
        label: "CodeBuddy（IDE / 插件）",
        skill_dir: &[".codebuddy", "skills"],
        mcp_file: &[".codebuddy", "mcp.json"],
        skill_root: Root::Home,
        mcp_root: Root::Home,
        style: McpStyle::CodeBuddy,
        note: "IDE 与插件版共用此目录",
    },
    Target {
        key: TargetKey::ZCode,
        label: "ZCode",
        skill_dir: &[".zcode", "skills"],
        mcp_file: &[".zcode", "cli", "config.json"],
        skill_root: Root::Home,
        mcp_root: Root::Home,
        style: McpStyle::ZCode,
        note: "MCP 在 mcp.servers 下，停用用 enabled:false",
    },
    Target {
        key: TargetKey::Doubao,
        label: "豆包",
        // %LOCALAPPDATA%\Doubao\User Data\Default\.doubao\agent_mode\workspace\.user_skills
        // `.skills` 是客户端下发的 41 个官方技能（会被升级覆盖），
        // `.user_skills` 才是给用户/外部工具放技能的位置——分发写这里。
        skill_root: Root::LocalData,
        skill_dir: &[
            "Doubao",
            "User Data",
            "Default",
            ".doubao",
            "agent_mode",
            "workspace",
            ".user_skills",
        ],
        // 豆包的 MCP 只在应用内管理，本地没有可写的配置文件：
        // 全盘搜过 User Data 与 Roaming，只有 Application\app\mcp_helper.dll。
        mcp_root: Root::LocalData,
        mcp_file: &[],
        style: McpStyle::Standard,
        note: "技能在 %LOCALAPPDATA%\\…\\.user_skills；MCP 只在应用内管理",
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
        TargetKey::Doubao => "doubao",
    }
}

fn root_path(root: Root, home: &Path, appdata: &Path, localdata: &Path) -> PathBuf {
    match root {
        Root::Home => home.to_path_buf(),
        Root::Appdata => appdata.to_path_buf(),
        Root::LocalData => localdata.to_path_buf(),
    }
}

/// 技能目录（`~/.qoder-cn/skills`、`%LOCALAPPDATA%\Doubao\…\.user_skills`）。
pub fn skill_dir(home: &Path, appdata: &Path, localdata: &Path, key: TargetKey) -> PathBuf {
    let target = get(key);
    target.skill_dir.iter().fold(
        root_path(target.skill_root, home, appdata, localdata),
        |path, part| path.join(part),
    )
}

/// MCP 配置文件（Trae 在 `%APPDATA%` 下，其余在家目录）。
/// **返回 None = 这个平台没有可写的本地 MCP 配置**，调用方必须跳过而不是写到家目录里。
pub fn mcp_file(home: &Path, appdata: &Path, localdata: &Path, key: TargetKey) -> Option<PathBuf> {
    let target = get(key);
    if !target.supports_mcp() {
        return None;
    }
    Some(target.mcp_file.iter().fold(
        root_path(target.mcp_root, home, appdata, localdata),
        |path, part| path.join(part),
    ))
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
            TargetKey::Doubao,
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
        let localdata = Path::new("C:/Users/x/AppData/Local");
        assert!(
            skill_dir(home, appdata, localdata, TargetKey::QoderCn).ends_with(".qoder-cn/skills")
        );
        assert!(skill_dir(home, appdata, localdata, TargetKey::TraeCn).ends_with(".trae-cn/skills"));
        assert!(skill_dir(home, appdata, localdata, TargetKey::WorkBuddy)
            .ends_with(".workbuddy/skills"));
        assert!(
            skill_dir(home, appdata, localdata, TargetKey::WorkBuddyIntl)
                .ends_with(".workbuddy-ai/skills")
        );
        assert!(mcp_file(home, appdata, localdata, TargetKey::TraeCn)
            .unwrap()
            .ends_with("Trae CN/User/mcp.json"));
        assert!(mcp_file(home, appdata, localdata, TargetKey::ZCode)
            .unwrap()
            .ends_with(".zcode/cli/config.json"));
    }

    /// 豆包的技能目录在 `%LOCALAPPDATA%` 下，不是家目录；MCP 没有可写文件。
    /// 这两条错一条都会静默失效：写到家目录里豆包看不见，写 MCP 会把家目录当配置文件。
    #[test]
    fn doubao_skills_live_in_localdata_and_have_no_mcp_file() {
        let home = Path::new("C:/Users/x");
        let appdata = Path::new("C:/Users/x/AppData/Roaming");
        let localdata = Path::new("C:/Users/x/AppData/Local");
        let dir = skill_dir(home, appdata, localdata, TargetKey::Doubao);
        assert!(
            dir.ends_with("Doubao/User Data/Default/.doubao/agent_mode/workspace/.user_skills"),
            "{}",
            dir.display()
        );
        assert!(
            dir.starts_with(localdata),
            "不能在家长里：{}",
            dir.display()
        );
        assert!(
            mcp_file(home, appdata, localdata, TargetKey::Doubao).is_none(),
            "豆包没有可写的本地 MCP 配置，不能返回一个路径"
        );
        assert!(!TARGETS
            .iter()
            .find(|t| t.key == TargetKey::Doubao)
            .unwrap()
            .supports_mcp());
    }

    /// 技能 / MCP 的部署目标按版本分开，显示名与产品表必须一致——
    /// 名字对不上时用户会以为「装到了国际版」，实际落在国内版目录。
    #[test]
    fn workbuddy_labels_match_the_product_table() {
        use crate::app::products::Product;
        for (key, product) in [
            (TargetKey::WorkBuddy, Product::WorkBuddyCn),
            (TargetKey::WorkBuddyIntl, Product::WorkBuddyIntl),
        ] {
            let target = TARGETS.iter().find(|t| t.key == key).unwrap();
            assert_eq!(target.label, product.label(), "{key:?} 显示名不一致");
        }
        // 两个版本的技能目录不能是同一个
        let home = Path::new("C:/Users/x");
        let appdata = Path::new("C:/Users/x/AppData/Roaming");
        let localdata = Path::new("C:/Users/x/AppData/Local");
        assert_ne!(
            skill_dir(home, appdata, localdata, TargetKey::WorkBuddy),
            skill_dir(home, appdata, localdata, TargetKey::WorkBuddyIntl)
        );
    }
}
