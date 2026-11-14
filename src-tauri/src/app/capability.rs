//! 平台能力矩阵：**一张表说清「哪家能做什么、哪家做不了」**。
//!
//! 这张表是唯一真源——界面只渲染它，不在前端另写一套 `if (platform === ...)`。
//! 目的很实际：用户看到「Trae 不能切换登录」时，能一眼分清这是**平台限制**
//! 还是**本应用还没做**，而不是反复点按钮试错。
//!
//! **维护纪律**：新增或改动任一平台能力，必须同步改这里的对应格子；
//! 格子里的 `note` 写「为什么不行 / 限制在哪」，不要写「暂不支持」这种空话。

use serde::Serialize;

/// 一格的支持程度。`Partial` = 能用但有限制（只读、需要客户端配合、只能借道）。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Full,
    Partial,
    None,
}

/// 一列：一个功能。`hint` 解释这个功能在本应用里具体指什么。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityFeature {
    pub key: String,
    pub label: String,
    pub hint: String,
}

/// 一格：状态 + 这一家的具体限制。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityCell {
    pub state: Support,
    pub note: String,
}

/// 一行：一家平台，`cells` 与 `features` 等长且同序。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityRow {
    pub key: String,
    pub label: String,
    pub cells: Vec<CapabilityCell>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityMatrix {
    pub features: Vec<CapabilityFeature>,
    pub platforms: Vec<CapabilityRow>,
}

// ------------------------------------------------------------------ 表本体

struct FeatureDef {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
}

const FEATURES: &[FeatureDef] = &[
    FeatureDef {
        key: "login",
        label: "浏览器登录",
        hint: "不打扰客户端：浏览器里完成授权后凭据自动入库",
    },
    FeatureDef {
        key: "scan",
        label: "扫描客户端",
        hint: "读客户端本地登录态入库；没装客户端返回 0 个账号，不算错误",
    },
    FeatureDef {
        key: "switch",
        label: "切换登录",
        hint: "把已保存的凭据写回客户端登录态",
    },
    FeatureDef {
        key: "checkin",
        label: "每日签到",
        hint: "调用平台签到接口，可自动可手动",
    },
    FeatureDef {
        key: "credits",
        label: "积分与余额",
        hint: "积分余额、发放批次或额度快照",
    },
    FeatureDef {
        key: "quota",
        label: "额度窗口",
        hint: "额度型平台按窗口显示已用百分比",
    },
    FeatureDef {
        key: "travel",
        label: "猫猫旅行",
        hint: "WorkBuddy 国内版的成长活动",
    },
    FeatureDef {
        key: "renew",
        label: "凭据刷新",
        hint: "每次重扫客户端，把最新凭据与有效期写回本地库；刷新后仍过期才需要重新登录",
    },
    FeatureDef {
        key: "sessions",
        label: "会话管理",
        hint: "浏览、归档、彻底删除客户端本地会话",
    },
    FeatureDef {
        key: "hub",
        label: "技能与 MCP",
        hint: "把技能与 MCP 配置分发到客户端",
    },
];

struct PlatformDef {
    key: &'static str,
    label: &'static str,
    /// 与 `FEATURES` 一一对应
    cells: &'static [(Support, &'static str)],
}

const fn full(note: &'static str) -> (Support, &'static str) {
    (Support::Full, note)
}
const fn part(note: &'static str) -> (Support, &'static str) {
    (Support::Partial, note)
}
const fn no(note: &'static str) -> (Support, &'static str) {
    (Support::None, note)
}

/// 刷新格子：四家都是同一条路，只有「客户端里也过期」这个共同限制
const REFRESH_NOTE: &str = "每次重扫客户端写回最新凭据与有效期；客户端里也过期就只能重新登录";

const PLATFORMS: &[PlatformDef] = &[
    PlatformDef {
        key: "workbuddy",
        label: "WorkBuddy 国内版",
        cells: &[
            full(""),
            full(""),
            full("整份登录态档案一起换，切换前自动备份；网页登录的账号没有档案，切不了"),
            full(""),
            full("资源包接口自带剩余量与到期时间"),
            no("签到型平台，没有额度窗口"),
            full(""),
            part(REFRESH_NOTE),
            full(""),
            full(""),
        ],
    },
    PlatformDef {
        key: "workbuddy_intl",
        label: "WorkBuddy 国际版",
        cells: &[
            full(""),
            full(""),
            full("整份登录态档案一起换，切换前自动备份；网页登录的账号没有档案，切不了"),
            no("国际版没有签到玩法，只查积分"),
            full(""),
            no("不是额度型平台"),
            no("猫猫旅行仅国内版有"),
            part(REFRESH_NOTE),
            full(""),
            full(""),
        ],
    },
    PlatformDef {
        key: "qoder",
        label: "Qoder CN",
        cells: &[
            full(""),
            full(""),
            full("重封 auth.v1.dat，不用换整份登录态"),
            full(""),
            full(""),
            no("按积分计，没有额度窗口"),
            no("非 WorkBuddy 平台"),
            part(REFRESH_NOTE),
            full(""),
            full(""),
        ],
    },
    PlatformDef {
        key: "qoder_intl",
        label: "Qoder 国际版",
        cells: &[
            full(""),
            full(""),
            full("重封 auth.v1.dat，不用换整份登录态"),
            full(""),
            full(""),
            no("按积分计，没有额度窗口"),
            no("非 WorkBuddy 平台"),
            part(REFRESH_NOTE),
            full(""),
            full(""),
        ],
    },
    PlatformDef {
        key: "zcode",
        label: "ZCode",
        cells: &[
            full(""),
            full(""),
            full("写回 config.json 与凭据条目"),
            no("没有签到玩法；活动奖励要在详情页手动领取"),
            full("额度型：显示各窗口已用百分比"),
            full(""),
            no("非 WorkBuddy 平台"),
            part(REFRESH_NOTE),
            full(""),
            full(""),
        ],
    },
    PlatformDef {
        key: "doubao",
        label: "豆包",
        cells: &[
            no("豆包登录走字节 SSO，没有可回调的授权码；改用「粘贴 Cookie」添加"),
            no("电脑版登录态在 Chromium 加密库里，格式随客户端版本变，不解析"),
            no("本工具不写豆包客户端的 Cookie 库，没有可回写的登录态"),
            full("个人账号的「天天领额度」，多账号各签各的"),
            part("显示累计体验额度；剩余次数以客户端为准"),
            no("按额度发放，不是窗口型"),
            no("非 WorkBuddy 平台"),
            part("Cookie 过期只能重新粘贴；有效期由字节控制，接口不返回"),
            no("会话在云端，本地没有可读的会话库"),
            no("豆包没有技能 / MCP 概念"),
        ],
    },
    PlatformDef {
        key: "trae",
        label: "Trae",
        cells: &[
            no("登录态与设备绑定，本应用只读"),
            full(""),
            no("加密格式无法安全写回，写回去会把客户端登出"),
            full(""),
            full("接口自带发放批次"),
            no("按积分计，没有额度窗口"),
            no("非 WorkBuddy 平台"),
            part("只能去 Trae 客户端登录一次，再点「扫描客户端」取新凭据"),
            part("会话在云端，本地只做墓碑与占位预览"),
            full(""),
        ],
    },
    PlatformDef {
        key: "codebuddy",
        label: "CodeBuddy",
        cells: &[
            no("账号体系跟随 WorkBuddy，不单独登录"),
            no("登录态来自 WorkBuddy 档案"),
            full("用 WorkBuddy 国内版档案切换 CodeBuddy IDE 与插件"),
            no("没有签到玩法"),
            no("本应用不采集它的积分"),
            no("不是额度型平台"),
            no("非 WorkBuddy 平台"),
            no("凭据由 WorkBuddy 侧维护"),
            full(""),
            full(""),
        ],
    },
];

/// 组装给界面用的矩阵。纯内存拼装，不碰磁盘。
pub fn matrix() -> CapabilityMatrix {
    CapabilityMatrix {
        features: FEATURES
            .iter()
            .map(|f| CapabilityFeature {
                key: f.key.to_string(),
                label: f.label.to_string(),
                hint: f.hint.to_string(),
            })
            .collect(),
        platforms: PLATFORMS
            .iter()
            .map(|p| CapabilityRow {
                key: p.key.to_string(),
                label: p.label.to_string(),
                cells: p
                    .cells
                    .iter()
                    .map(|(state, note)| CapabilityCell {
                        state: *state,
                        note: note.to_string(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_row_covers_every_feature() {
        // 少一格界面就会错位：这条断言是矩阵本身的护栏
        for platform in PLATFORMS {
            assert_eq!(
                platform.cells.len(),
                FEATURES.len(),
                "{} 的格子数与功能数不一致",
                platform.label
            );
        }
    }

    #[test]
    fn unsupported_cells_explain_themselves() {
        // 「不支持」必须带原因，否则界面只能显示一个没有信息量的叉
        for platform in PLATFORMS {
            for (index, (state, note)) in platform.cells.iter().enumerate() {
                if *state == Support::None {
                    assert!(
                        !note.is_empty(),
                        "{} 的「{}」缺说明",
                        platform.label,
                        FEATURES[index].label
                    );
                }
            }
        }
    }

    #[test]
    fn platform_keys_are_unique() {
        let mut keys: Vec<&str> = PLATFORMS.iter().map(|p| p.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len());
    }

    #[test]
    fn matrix_is_serializable_with_matching_shape() {
        let m = matrix();
        assert_eq!(m.features.len(), FEATURES.len());
        assert!(m
            .platforms
            .iter()
            .all(|p| p.cells.len() == m.features.len()));
    }
}
