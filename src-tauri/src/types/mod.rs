//! 领域类型。P0 只定字段，不含逻辑。
//!
//! 两条硬性设计约束：
//!   1. `Account` **故意不设 token 字段**——凭据无法被序列化出去，这是类型层面的防泄露，
//!      对应 v1 里 `meta` 自由 dict 装着 refresh_token 的教训。
//!   2. `iid` 是四处共用的主键，允许作为 key 存在，但界面不得渲染（契约 17）。

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    pub checked_in: Option<bool>,
    pub summary: String,
    pub balance: Option<f64>,
    pub lots: Option<Vec<CreditLot>>,
    /// 下次可签时刻，**只由接口下发**，不用本地时钟推算（契约 3）
    pub next_at: Option<DateTime<Local>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditSnapshot {
    pub summary: String,
    pub balance: Option<f64>,
    pub lots: Option<Vec<CreditLot>>,
    /// 额度型平台：窗口名 → 已用百分比
    pub quota: Option<HashMap<String, f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub ok: bool,
    pub message: String,
    pub granted: Option<f64>,
    pub validity_days: Option<i64>,
    /// 本轮早领过：不算失败，但也**不算今日已签**
    pub already: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Visible,
    Deleted,
    OldAccount,
    Cloud,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub kind: String,
    pub source_key: String,
    pub source_label: String,
    pub account: String,
    pub session_id: String,
    pub title: String,
    pub last_at: Option<DateTime<Local>>,
    pub msgs: Option<i64>,
    pub size: Option<i64>,
    pub state: SessionState,
    pub has_body: bool,
}

/// 游标分页结果。会话类必须用它；账号 / 技能数量天然有限，直接返回 Vec。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub total: usize,
}
