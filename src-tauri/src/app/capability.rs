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

/// 一家平台的自动刷新节奏。`min_minutes = 0` 表示跟随全局「积分刷新」任务。
///
/// 数值与说明都来自 Provider 自己（`credits_min_interval_minutes` /
/// `credits_interval_note`）：接口性质只有适配器最清楚，界面与调度器读同一份。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct RefreshPolicy {
    pub platform: String,
    pub label: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub min_minutes: u64,
    pub note: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityMatrix {
    pub features: Vec<CapabilityFeature>,
    pub platforms: Vec<CapabilityRow>,
    /// 自动刷新节奏：与 `platforms` 同一批平台，供「定时任务」页解释平台差异
    pub refresh_policies: Vec<RefreshPolicy>,
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
        hint: "先问云端（用保存的凭据打只读接口），再重扫客户端取最新凭据与有效期；两边都拿不到才需要重新授权",
    },
    FeatureDef {
        key: "sessions",
        label: "会话管理",
        hint: "浏览、归档、彻底删除客户端本地会话",
    },
    FeatureDef {
        key: "restore",
        label: "恢复对话",
        hint: "把本机历史会话的归属改到当前登录账号：只改引用，正文一个字节都不动",
    },
    FeatureDef {
        key: "hub",
        label: "技能与 MCP",
        hint: "把技能与 MCP 配置分发到客户端",
    },
    FeatureDef {
        key: "switch_ide",
        label: "切换 IDE",
        hint: "把账号的登录态写进 CodeBuddy IDE；只对 WorkBuddy 国内版账号有效",
    },
    FeatureDef {
        key: "switch_idea",
        label: "切换插件",
        hint: "把账号的登录态写进 IntelliJ IDEA 里的 CodeBuddy 插件；同上",
    },
    FeatureDef {
        key: "cloud",
        label: "云端校验",
        hint: "刷新状态/凭据时，先用保存的凭据问一次平台（只读余额接口）；云端给不出答案才回退读本机客户端",
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
const REFRESH_NOTE: &str = "每次重扫客户端写回最新凭据与有效期；客户端里也过期就只能重新授权";

/// 切换登录的对外口径：只对 WorkBuddy（含 CodeBuddy 系）与 ZCode 开放。
/// 页面上的「切换账号」图标、后端预检与设置页开关都读这一条，不再各写一套名单。
const SWITCH_SCOPE_NOTE: &str =
    "切换登录只对 WorkBuddy（含 CodeBuddy 系）与 ZCode 开放；本平台只做扫描、签到与积分查询";

/// ima 的计量单位是「算力」，不能套用其他平台的「积分」口径。
const SWITCH_IMA_NOTE: &str =
    "切换登录只对 WorkBuddy（含 CodeBuddy 系）与 ZCode 开放；ima 只做扫描、签到与算力查询";

/// 会话恢复改的是「会话归属」：只有 WorkBuddy 的本地会话库带账号字段。
/// Qoder / ZCode 虽然也有本地库，但库不区分账号；豆包 / ima / Trae 的会话在云端。
const RESTORE_SCOPE_NOTE: &str =
    "只有 WorkBuddy 的本地会话库带账号归属；本平台没有可改写归属的本地会话库";

/// ZCode 有本地会话库，但换号共用一份库：历史会话本来就全部可见，没有需要改写的归属。
const ZCODE_RESTORE_NOTE: &str =
    "ZCode 的会话库不区分账号（换号共用一份库），历史会话换号后仍然全部可见，无需恢复";

/// CodeBuddy 系两个客户端（IDE / IDEA 插件）共用同一个写入来源：
/// 只有 WorkBuddy 国内版账号的档案能写进去。其他平台在这一列必须写清「不支持」。
const CB_SCOPE_NOTE: &str = "只有 WorkBuddy 国内版账号能写入 CodeBuddy 系客户端";

/// 国际版连写入口都没有：CodeBuddy 系没有国际版授权域。
const CB_INTL_NOTE: &str = "CodeBuddy 系没有国际版授权域，切过去也登不上";

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
            full("把本机历史会话的归属改到当前登录账号，只改引用不动正文"),
            full(""),
            full("用国内版账号档案写入 CodeBuddy IDE 的登录态"),
            full("用国内版账号档案写入 IDEA 插件的登录态"),
            part("先问资源包接口校验凭据；令牌续期协议未公开，过期仍需客户端或网页重新授权"),
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
            full("同国内版，按国际版自己的会话库处理"),
            full(""),
            no(CB_INTL_NOTE),
            no(CB_INTL_NOTE),
            part("先问资源包接口校验凭据；国际版令牌续期同样未公开，过期需重新授权"),
        ],
    },
    PlatformDef {
        key: "qoder",
        label: "Qoder CN",
        cells: &[
            full(""),
            full(""),
            no(SWITCH_SCOPE_NOTE),
            full(""),
            full(""),
            no("按积分计，没有额度窗口"),
            no("非 WorkBuddy 平台"),
            part(REFRESH_NOTE),
            full(""),
            no(RESTORE_SCOPE_NOTE),
            full(""),
            no(CB_SCOPE_NOTE),
            no(CB_SCOPE_NOTE),
            full("先问配额接口校验凭据；账号有效期取 refresh token，不受访问令牌每天续期影响"),
        ],
    },
    PlatformDef {
        key: "qoder_intl",
        label: "Qoder 国际版",
        cells: &[
            full(""),
            full(""),
            no(SWITCH_SCOPE_NOTE),
            full(""),
            full(""),
            no("按积分计，没有额度窗口"),
            no("非 WorkBuddy 平台"),
            part(REFRESH_NOTE),
            full(""),
            no(RESTORE_SCOPE_NOTE),
            full(""),
            no(CB_SCOPE_NOTE),
            no(CB_SCOPE_NOTE),
            full("先问配额接口校验凭据；账号有效期取 refresh token，不受访问令牌每天续期影响"),
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
            no(ZCODE_RESTORE_NOTE),
            full(""),
            no(CB_SCOPE_NOTE),
            no(CB_SCOPE_NOTE),
            full("先问额度接口校验凭据；额度窗口本身就来自云端，不读本地缓存"),
        ],
    },
    PlatformDef {
        key: "doubao",
        label: "豆包",
        cells: &[
            no("豆包登录走字节 SSO，没有可回调的授权码；改用「粘贴 Cookie」添加"),
            no("电脑版登录态在 Chromium 加密库里，格式随客户端版本变，不解析"),
            no("本工具不写豆包客户端的 Cookie 库，没有可回写的登录态"),
            no("豆包领取接口会被风控，本工具只做只读积分查询"),
            part("显示累计体验额度；剩余次数以客户端为准"),
            no("按额度发放，不是窗口型"),
            no("非 WorkBuddy 平台"),
            part("Cookie 过期只能重新粘贴；有效期由字节控制，接口不返回"),
            no("会话在云端，本地没有可读的会话库"),
            no(RESTORE_SCOPE_NOTE),
            part(
                "技能可分发到 %LOCALAPPDATA%\\...\\.user_skills；MCP 只在应用内管理，没有可写配置",
            ),
            no(CB_SCOPE_NOTE),
            no(CB_SCOPE_NOTE),
            part("先问额度接口校验 Cookie；有效期接口不返回，过期时间仍按本地记录提醒"),
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
            no(RESTORE_SCOPE_NOTE),
            full(""),
            no(CB_SCOPE_NOTE),
            no(CB_SCOPE_NOTE),
            part("先问积分接口校验凭据；令牌续期只能在客户端里完成，过期需重新登录"),
        ],
    },
    PlatformDef {
        key: "ima",
        label: "ima",
        cells: &[
            no("登录态只在 ima 客户端里，本应用不提供浏览器授权"),
            full(""),
            no(SWITCH_IMA_NOTE),
            full(""),
            full("显示签到累计的算力余额；平台只回累计值，没有发放批次"),
            no("按算力计，没有额度窗口"),
            no("非 WorkBuddy 平台"),
            part("每次重扫客户端写回最新 refresh token；refresh token 失效只能打开 ima 客户端重新登录"),
            no("会话在云端，本地没有可读的会话库"),
            no(RESTORE_SCOPE_NOTE),
            no("ima 客户端没有可写的技能与 MCP 配置"),
            no(CB_SCOPE_NOTE),
            no(CB_SCOPE_NOTE),
            part("先刷新访问令牌再查签到活动校验凭据；访问令牌约 2 小时有效，由本地 refresh token 续期，续期失败需重新登录"),
        ],
    },
    PlatformDef {
        key: "codebuddy",
        label: "CodeBuddy 系（IDE / 插件）",
        cells: &[
            no("与 WorkBuddy 同一账号体系，不单独登录"),
            no("登录态来自 WorkBuddy 档案"),
            full("用 WorkBuddy 国内版档案切换 CodeBuddy IDE 与 IDEA 插件"),
            no("没有签到玩法"),
            no("不单独采集：三端共用 WorkBuddy 的积分池，重复记一次就多算"),
            no("不是额度型平台"),
            no("非 WorkBuddy 平台"),
            no("凭据由 WorkBuddy 侧维护"),
            full(""),
            no("会话恢复跟着 WorkBuddy 账号走，不按客户端单独提供"),
            full(""),
            full("接收方：用 WorkBuddy 国内版账号档案写入 CodeBuddy IDE"),
            full("接收方：用 WorkBuddy 国内版账号档案写入 IDEA 插件"),
            no("账号体系跟随 WorkBuddy，不单独查云端；校验与刷新都在 WorkBuddy 那一行走"),
        ],
    },
];

/// 查一格：某平台某功能到底有多大支持度（附带原因）。
///
/// 平台功能开关（`app::features`）靠它判断「这个功能在这家能不能开」——
/// 表里写「不支持」的，设置页里连勾选项都不给。
pub fn support_of(platform: &str, feature: &str) -> Option<(Support, &'static str)> {
    let row = PLATFORMS.iter().find(|p| p.key == platform)?;
    let index = FEATURES.iter().position(|f| f.key == feature)?;
    row.cells.get(index).copied()
}

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
        // 节奏由适配器自己声明：接口是「轻量余额 GET」还是「整块活动面板」，
        // 只有写适配器的人知道；这里只负责汇总给界面。
        refresh_policies: crate::app::providers::all()
            .iter()
            .map(|provider| RefreshPolicy {
                platform: provider.platform().to_string(),
                label: provider.label().to_string(),
                min_minutes: provider.credits_min_interval_minutes(),
                note: provider.credits_interval_note().to_string(),
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
    fn switch_only_opens_for_workbuddy_codebuddy_and_zcode() {
        // 产品口径：切换登录只对 WorkBuddy（含 CodeBuddy 系）与 ZCode 开放。
        // 界面图标、后端预检、设置页开关都读这一格，写错一处就会出现点不动的按钮。
        for key in ["workbuddy", "workbuddy_intl", "zcode", "codebuddy"] {
            let (state, _) = support_of(key, "switch").expect("缺 switch 格");
            assert_eq!(state, Support::Full, "{key} 应支持切换");
        }
        for key in ["qoder", "qoder_intl", "trae", "doubao", "ima"] {
            let (state, note) = support_of(key, "switch").expect("缺 switch 格");
            assert_eq!(state, Support::None, "{key} 不该再宣称能切换");
            assert!(!note.is_empty(), "{key} 的切换限制要写清原因");
        }
    }

    #[test]
    fn restore_belongs_to_workbuddy_only() {
        // 会话库只存在于 WorkBuddy；其他平台写「支持」等于让用户点一个必然失败的按钮
        for key in ["workbuddy", "workbuddy_intl"] {
            let (state, _) = support_of(key, "restore").expect("缺 restore 格");
            assert_eq!(state, Support::Full, "{key} 应支持恢复对话");
        }
        for key in [
            "qoder",
            "qoder_intl",
            "zcode",
            "trae",
            "doubao",
            "codebuddy",
        ] {
            let (state, note) = support_of(key, "restore").expect("缺 restore 格");
            assert_eq!(state, Support::None, "{key} 不该支持恢复对话");
            assert!(!note.is_empty(), "{key} 缺恢复对话的原因");
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

    #[test]
    fn refresh_policies_cover_every_provider_and_doubao_is_throttled() {
        let m = matrix();
        // 每个 Provider 都要有一行：设置页的说明不能漏掉某一家
        assert_eq!(
            m.refresh_policies.len(),
            crate::app::providers::all().len(),
            "刷新节奏行数与 Provider 数不一致"
        );
        let doubao = m
            .refresh_policies
            .iter()
            .find(|p| p.platform == "doubao")
            .expect("豆包缺刷新节奏");
        assert_eq!(doubao.min_minutes, 30);
        assert!(
            doubao.note.contains("活动面板"),
            "豆包要写清为什么慢：{}",
            doubao.note
        );
        // 其余平台跟随全局：0 且不需要额外说明
        assert!(m
            .refresh_policies
            .iter()
            .filter(|p| p.platform != "doubao")
            .all(|p| p.min_minutes == 0 && p.note.is_empty()));
    }
}
