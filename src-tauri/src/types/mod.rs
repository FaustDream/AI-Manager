//! 领域类型。P0 只定字段，不含逻辑。
//!
//! 两条硬性设计约束：
//!   1. `Account` **故意不设 token 字段**——凭据无法被序列化出去，这是类型层面的防泄露，
//!      对应 v1 里 `meta` 自由 dict 装着 refresh_token 的教训。
//!   2. `iid` 是四处共用的主键，允许作为 key 存在，但界面不得渲染（契约 17）。

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKey {
    TraeCn,
    Trae,
    QoderCn,
    QoderIntl,
    WorkBuddy,
    WorkBuddyIntl,
    ZCode,
}

/// 积分数字展示：整数值不带小数点，其余原样。
/// 三家平台适配器各写过一份，第三次照写前提到了这里。
pub fn trim_f64(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// "{platform}:{account_id}"，账号库 / 台账 / 签到记录 / UI 行 key 共用
    pub iid: String,
    pub platform: PlatformKey,
    pub platform_label: String,
    pub account_id: String,
    /// 昵称；手机号形态由上层脱敏后再展示
    pub name: String,
    pub expires_at: Option<DateTime<Local>>,
    pub source: String,
    pub requires_checkin: bool,
    /// 只给键名不给值——值里可能躺着 refresh_token / 手机号 / 邮箱
    pub meta_keys: Vec<String>,
}

impl Account {
    pub fn iid_of(platform: PlatformKey, account_id: &str) -> String {
        format!("{platform:?}:{account_id}").to_lowercase()
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditLot {
    pub amount: f64,
    /// 到期日，接口给
    pub expires_at: Option<DateTime<Local>>,
    /// 发放时刻，本地记（接口不含，契约 5b）
    pub granted_at: Option<DateTime<Local>>,
    /// 本地生成的去重键
    pub lot_key: String,
    pub label: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    pub checked_in: Option<bool>,
    pub summary: String,
    pub balance: Option<f64>,
    pub lots: Option<Vec<CreditLot>>,
    /// 下次可签时刻，**只由接口下发**，不用本地时钟推算（契约 3）
    pub next_at: Option<DateTime<Local>>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditSnapshot {
    pub summary: String,
    /// 本次查询失败的原因；None = 查询成功，可能只是没拿到数值。
    /// 界面不能靠「余额与配额都为空」反推失败：额度型平台成功时也可能两者都为空。
    pub error: Option<String>,
    pub balance: Option<f64>,
    pub lots: Option<Vec<CreditLot>>,
    /// 额度型平台：窗口名 → 已用百分比
    pub quota: Option<HashMap<String, f64>>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub ok: bool,
    pub message: String,
    pub granted: Option<f64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub validity_days: Option<i64>,
    /// 本轮早领过：不算失败，但也**不算今日已签**
    pub already: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Visible,
    Deleted,
    /// 归属账号不是当前登录的那个：客户端里还在，但当前账号的列表看不到
    OldAccount,
    /// 只在服务端：本地没有正文，删不掉
    Cloud,
    /// 客户端里被归档：不显示在默认列表，但数据还在
    Archived,
    /// 父会话已从库里消失的内部会话（ZCode 子代理 / IDE 侧聊）：正文还在，
    /// 但客户端列表与主会话都指不到它，不扫出来就成了三不管数据
    Orphan,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub kind: String,
    pub source_key: String,
    pub source_label: String,
    pub account: String,
    /// 界面显示用昵称（与账号库匹配）；uid 原文只留给行 key 与筛选
    pub account_label: String,
    pub session_id: String,
    pub title: String,
    /// 所属项目：优先工作区名（slug），其次 cwd 的最后一段；都没有就是空串
    pub project: String,
    /// 项目完整路径（界面放在 title 属性里，不占列宽）
    pub cwd: String,
    /// 运行形态：`desktop` / `ide-plugin`，用于 IDEA 插件标签聚合
    pub origin: String,
    pub last_at: Option<DateTime<Local>>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub msgs: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub size: Option<i64>,
    pub state: SessionState,
    pub has_body: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 游标分页结果。会话类必须用它；账号 / 技能数量天然有限，直接返回 Vec。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
}
