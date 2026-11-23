//! 平台功能开关：**哪个软件的哪个功能出现在界面上，由用户在设置页勾，页面里不再写死**。
//!
//! 两层判据，缺一不可：
//!   - `available`：平台到底有没有这个能力 —— 唯一来源是 `capability.rs` 的能力表；
//!   - `enabled`：用户在当前设备的勾选 —— 存在 `data/config.toml` 的 `[features.<平台>]`。
//!
//! 能力表说「不支持」的格子不给勾（勾了也是假的，按钮点下去只会报错）；用户关掉的
//! 只影响界面入口，不改平台能力、也不停掉已经排好的定时任务。所以这里是**显示开关**，
//! 不是安全边界——真正做不做得到由各适配器自己的卫语句决定。

use crate::app::capability::{self, Support};
use crate::app::config;
use serde::Serialize;

/// 功能键：与 `capability.rs` 的 FEATURES 同名，改一处必须一起改。
pub const CHECKIN: &str = "checkin";
pub const SWITCH: &str = "switch";
/// 切换 CodeBuddy IDE 的登录态（只有 WorkBuddy 国内版账号能写）
pub const SWITCH_IDE: &str = "switch_ide";
/// 切换 IntelliJ IDEA 里 CodeBuddy 插件的登录态（同上）
pub const SWITCH_IDEA: &str = "switch_idea";
pub const RESTORE: &str = "restore";
pub const TRAVEL: &str = "travel";

/// 一列：一个可配置的功能。
pub struct FeatureDef {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

/// 用户可配的功能。顺序 = 设置页的列顺序。
const FEATURES: &[FeatureDef] = &[
    FeatureDef {
        key: CHECKIN,
        label: "显示签到",
        hint: "账号中心与详情页的签到按钮、列表签到列；关掉后自动签到仍按定时任务跑",
    },
    FeatureDef {
        key: SWITCH,
        label: "切换账号",
        hint: "把已保存的凭据写回客户端登录态；CodeBuddy 系按 IDE / IDEA 插件分开计",
    },
    FeatureDef {
        key: SWITCH_IDE,
        label: "切换 IDE",
        hint: "把账号的登录态写进 CodeBuddy IDE；只对 WorkBuddy 国内版账号有效",
    },
    FeatureDef {
        key: SWITCH_IDEA,
        label: "切换插件",
        hint: "把账号的登录态写进 IntelliJ IDEA 里的 CodeBuddy 插件；同上",
    },
    FeatureDef {
        key: RESTORE,
        label: "恢复对话",
        hint: "把本机历史会话的归属改到当前登录账号：只改引用，正文不动",
    },
    FeatureDef {
        key: TRAVEL,
        label: "猫猫旅行",
        hint: "详情页的旅行卡片；只有 WorkBuddy 国内版有这个玩法",
    },
];

/// 一家平台（软件）。`capability` 指回能力表的行：CodeBuddy IDE 与 IDEA 插件共用
/// `codebuddy` 那一行，但界面要分开给开关——它们的登录态各写各的。
struct PlatformDef {
    key: &'static str,
    label: &'static str,
    capability: &'static str,
}

/// 顺序 = 设置页的行顺序 = 用户熟悉的产品顺序。
const PLATFORMS: &[PlatformDef] = &[
    PlatformDef {
        key: "workbuddy",
        label: "WorkBuddy 国内版",
        capability: "workbuddy",
    },
    PlatformDef {
        key: "workbuddy_intl",
        label: "WorkBuddy 国际版",
        capability: "workbuddy_intl",
    },
    // CodeBuddy IDE / CodeBuddy 插件（IDEA）不再单独列成一行：它们的「切换 IDE / 切换插件」
    // 入口已经嵌进 WorkBuddy 国内版账号的详情，这里只保留这两列，供用户按「workbuddy」这一行开关。
    PlatformDef {
        key: "zcode",
        label: "ZCode",
        capability: "zcode",
    },
    PlatformDef {
        key: "qoder",
        label: "Qoder CN",
        capability: "qoder",
    },
    PlatformDef {
        key: "qoder_intl",
        label: "Qoder 国际版",
        capability: "qoder_intl",
    },
    PlatformDef {
        key: "trae",
        label: "Trae",
        capability: "trae",
    },
    PlatformDef {
        key: "doubao",
        label: "豆包",
        capability: "doubao",
    },
];

// ------------------------------------------------------------------ 给界面的形状

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct FeatureColumn {
    pub key: String,
    pub label: String,
    pub hint: String,
}

/// 一格：平台有没有这个能力 + 用户有没有关掉。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct FeatureCell {
    /// 平台有没有这个能力（能力表判据）。false 时设置页只显示原因，不给勾。
    pub available: bool,
    /// 界面最终是否显示 = available && 用户没关。
    pub enabled: bool,
    /// 不可用的原因（能力表原文）；可用时为空。
    pub note: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct FeatureRow {
    pub key: String,
    pub label: String,
    /// 与 `features` 等长且同序
    pub cells: Vec<FeatureCell>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct FeatureMatrix {
    pub features: Vec<FeatureColumn>,
    pub platforms: Vec<FeatureRow>,
}

// ------------------------------------------------------------------ 判据

/// 功能表的平台键 → 平台定义。**能力表查询必须先过这一步**：
/// 两张表的键统一经 `capability` 字段映射，直接拿功能表键查能力表可能查不到。
fn platform_def(key: &str) -> Option<&'static PlatformDef> {
    PLATFORMS.iter().find(|p| p.key == key)
}

/// 平台本身有没有这个能力。`Partial` 也算有（能用但有限制，限制写在能力表里）。
/// `platform` 是功能表的平台键（`workbuddy` / `zcode` …）。
pub fn available(platform: &str, feature: &str) -> bool {
    let Some(def) = platform_def(platform) else {
        return false;
    };
    matches!(
        capability::support_of(def.capability, feature),
        Some((Support::Full, _)) | Some((Support::Partial, _))
    )
}

/// 不可用的原因：直接复用能力表的原文，界面不再另写一套解释。
fn note_of(platform: &str, feature: &str) -> String {
    match platform_def(platform).and_then(|def| capability::support_of(def.capability, feature)) {
        Some((Support::None, note)) => note.to_string(),
        _ => String::new(),
    }
}

/// 一层解析：用户没配过就是「开」。抽出来是为了能在测试里注入覆盖值，
/// 不必去动全局配置或真实 `data/`。
fn resolve(overrides: &config::FeatureOverrides, platform: &str, feature: &str) -> bool {
    if !available(platform, feature) {
        return false;
    }
    overrides
        .get(platform)
        .and_then(|row| row.get(feature))
        .copied()
        .unwrap_or(true)
}

/// 界面入口要不要渲染。平台不支持一律 false，用户关掉也是 false。
pub fn enabled(platform: &str, feature: &str) -> bool {
    resolve(&config::get().features, platform, feature)
}

/// 账号 iid → 功能表的平台键。WorkBuddy 两版共用 `workbuddy` 前缀，靠 `@intl` 后缀区分。
pub fn platform_of_iid(iid: &str) -> &str {
    let platform = iid.split(':').next().unwrap_or("");
    if platform == "workbuddy" && crate::app::accounts::is_intl_account(iid) {
        return "workbuddy_intl";
    }
    platform
}

/// 设置页的矩阵：可用性来自能力表，勾选来自配置。
pub fn matrix() -> FeatureMatrix {
    let overrides = config::get().features;
    FeatureMatrix {
        features: FEATURES
            .iter()
            .map(|f| FeatureColumn {
                key: f.key.to_string(),
                label: f.label.to_string(),
                hint: f.hint.to_string(),
            })
            .collect(),
        platforms: PLATFORMS
            .iter()
            .map(|p| FeatureRow {
                key: p.key.to_string(),
                label: p.label.to_string(),
                cells: FEATURES
                    .iter()
                    .map(|f| FeatureCell {
                        available: available(p.key, f.key),
                        enabled: resolve(&overrides, p.key, f.key),
                        note: note_of(p.key, f.key),
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn overrides(platform: &str, feature: &str, value: bool) -> config::FeatureOverrides {
        BTreeMap::from([(
            platform.to_string(),
            BTreeMap::from([(feature.to_string(), value)]),
        )])
    }

    #[test]
    fn defaults_follow_the_capability_table() {
        let empty = config::FeatureOverrides::new();
        assert!(resolve(&empty, "workbuddy", SWITCH));
        assert!(resolve(&empty, "zcode", SWITCH));
        // 平台做不到的，默认必须是关——默认开等于给用户一个必然失败的按钮
        assert!(!resolve(&empty, "qoder", SWITCH));
        assert!(!resolve(&empty, "doubao", CHECKIN));
        assert!(!resolve(&empty, "zcode", RESTORE));
        assert!(!resolve(&empty, "workbuddy_intl", TRAVEL));
    }

    #[test]
    fn user_override_only_touches_its_own_cell() {
        let o = overrides("workbuddy", SWITCH, false);
        assert!(!resolve(&o, "workbuddy", SWITCH));
        assert!(resolve(&o, "workbuddy", CHECKIN), "关一个功能不该牵连别的");
    }

    #[test]
    fn override_cannot_open_what_the_platform_lacks() {
        // 配置里写了 true 也不能把能力表里「不支持」的格子点亮
        let o = overrides("qoder", SWITCH, true);
        assert!(!resolve(&o, "qoder", SWITCH));
    }

    #[test]
    fn switch_row_matches_the_product_decision() {
        let m = matrix();
        let switch_index = m
            .features
            .iter()
            .position(|f| f.key == SWITCH)
            .expect("缺切换账号列");
        let enabled: Vec<&str> = m
            .platforms
            .iter()
            .filter(|p| p.cells[switch_index].available)
            .map(|p| p.key.as_str())
            .collect();
        assert_eq!(
            enabled,
            vec!["workbuddy", "workbuddy_intl", "zcode"],
            "切换账号只对 WorkBuddy（含 CodeBuddy 系）与 ZCode 开放"
        );
    }

    /// 界面看的是 `enabled` 而不是 `available`：CodeBuddy IDE / IDEA 插件不再单独列行，
    /// 切换入口落到 WorkBuddy 国内版这一行上。
    #[test]
    fn switch_row_is_enabled_for_the_supported_softwares() {
        let m = matrix();
        let index = m
            .features
            .iter()
            .position(|f| f.key == SWITCH)
            .expect("缺切换账号列");
        let enabled: Vec<&str> = m
            .platforms
            .iter()
            .filter(|p| p.cells[index].enabled)
            .map(|p| p.key.as_str())
            .collect();
        assert_eq!(
            enabled,
            vec!["workbuddy", "workbuddy_intl", "zcode"],
            "切换入口必须在这些软件上默认可见"
        );
    }

    /// 「切换 IDE / 切换插件」两列保留，但只对 WorkBuddy 国内版这一行开放——
    /// 入口已经嵌进它的账号详情，别的软件必须显示「平台不支持」。
    #[test]
    fn codebuddy_client_switches_only_open_on_the_workbuddy_row() {
        let m = matrix();
        for feature in [SWITCH_IDE, SWITCH_IDEA] {
            let index = m
                .features
                .iter()
                .position(|f| f.key == feature)
                .expect("缺列");
            let enabled: Vec<&str> = m
                .platforms
                .iter()
                .filter(|p| p.cells[index].enabled)
                .map(|p| p.key.as_str())
                .collect();
            assert_eq!(
                enabled,
                vec!["workbuddy"],
                "{feature} 只对 WorkBuddy 国内版开放"
            );
        }
    }

    #[test]
    fn every_row_is_full_width_and_unique() {
        let m = matrix();
        let mut keys: Vec<&str> = m.platforms.iter().map(|p| p.key.as_str()).collect();
        assert!(m
            .platforms
            .iter()
            .all(|p| p.cells.len() == m.features.len()));
        let before = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(before, keys.len(), "平台键重复");
    }

    #[test]
    fn unavailable_cells_explain_themselves() {
        for row in matrix().platforms {
            for (index, cell) in row.cells.iter().enumerate() {
                if !cell.available {
                    assert!(
                        !cell.note.is_empty(),
                        "{} 的「{}」不可用却没说原因",
                        row.label,
                        index
                    );
                }
            }
        }
    }

    #[test]
    fn iid_maps_workbuddy_editions_apart() {
        assert_eq!(platform_of_iid("workbuddy:u1@intl"), "workbuddy_intl");
        assert_eq!(platform_of_iid("workbuddy:u1"), "workbuddy");
        assert_eq!(platform_of_iid("zcode:abc"), "zcode");
        assert_eq!(platform_of_iid(""), "");
    }
}
