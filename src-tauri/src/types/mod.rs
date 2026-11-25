//! 领域类型。P0 只定字段，不含逻辑。
//!
//! 两条硬性设计约束：
//!   1. `Account` **故意不设 token 字段**——凭据无法被序列化出去，这是类型层面的防泄露，
//!      凭据字段不能被整体序列化出去。
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

// ---------------------------------------------------------------- 凭据形态与登录态
//
// 各平台的登录态**不是一回事**：WorkBuddy 是一份可切换的档案包、豆包是用户粘贴的
// 浏览器 Cookie、Trae 是设备绑定且只读、ima 只存 refresh token 每次重换、
// ZCode 的令牌根本不含到期时间。把这些压成一个「到期日」列，读起来全一样，
// 用户分不清哪个号能切、哪个号只是网页登录。
//
// 所以形态由**各平台自己声明**（Provider::credential_shape），
// 状态由 accounts::login_state 统一计算，界面只渲染，不猜。

/// 凭据的物理形态。决定「能不能切换」「要不要重扫」「到期时间可不可信」。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    /// 客户端自己写的令牌文件（Qoder / ZCode）
    ClientToken,
    /// 整套会话档案包，一份 = 一个账号（WorkBuddy 含 CodeBuddy 系）
    ProfilePack,
    /// 用户从浏览器粘贴的 Cookie，一份 = 一个账号（豆包）
    PastedCookie,
    /// 设备绑定的加密登录态，只能读不能写回（Trae）
    DeviceBound,
    /// 只存 refresh token，短期令牌每次请求重换（ima）
    RefreshCookie,
}

/// 一个平台的凭据口径。**静态声明，不做 I/O**。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialShape {
    pub kind: CredentialKind,
    /// 人类可读的形态名，进列表第二行
    pub label: String,
    /// 该平台是否「一份凭据 = 一个账号」（多账号并存，互不覆盖）
    pub multi_account: bool,
    /// 本工具能否把它写回客户端完成一键切换
    pub switchable: bool,
    /// 平台是否下发可信的到期时间。false 时到期列只能说「未知」，不是「没登录」
    pub expiry_known: bool,
    /// 口径说明，进 tooltip
    pub note: String,
}

/// 登录态结论。**按平台口径算出来的**，界面直接用。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStateKind {
    /// 这就是客户端当前登录的号
    Current,
    /// 凭据在本机、有效，但不是当前登录
    Saved,
    /// 3 天内到期
    ExpiringSoon,
    /// 凭据已过期，需要在客户端重新登录一次
    Expired,
    /// 凭据在，但平台不下发到期时间（ZCode / ima / 豆包）
    UnknownExpiry,
    /// 本机没有凭据：不能切换，也不参与自动签到
    NotSaved,
    /// 最近一次查询因凭据问题失败（登录失效等，不含网络/限流）
    Failed,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountLoginState {
    pub kind: LoginStateKind,
    /// 主行结论文案
    pub label: String,
    /// 悬停详情：为什么是这个结论、下一步做什么
    pub detail: String,
    pub shape: CredentialShape,
    /// 是否客户端当前登录
    pub current: bool,
    /// 现在能不能一键切换（形态支持 **且** 通过切换预检）
    pub switchable: bool,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires_at: Option<i64>,
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
    /// 今天签没签——**只有本平台这一发积分请求已经顺带看到结论时才填**。
    ///
    /// 各平台的接口切分不一样，这里必须按平台隔离，不能统一去「顺便查一下状态」：
    /// ima 的算力与签到由同一发活动面板回答，丢掉结论等于白付一次请求；
    /// Trae / Qoder / WorkBuddy 的积分接口与签到状态接口是分开的，
    /// 为了填这个字段去打状态端点，就把高频积分刷新变成了状态端点的脚本样本。
    /// 那几家一律留 `None`，已签结论由真实签到或本地去重表回答。
    #[serde(default)]
    pub checked_in: Option<bool>,
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
    /// 置顶标记（本地 marks，扫描重建不丢）。列表按置顶优先排序。
    pub pinned: bool,
    /// 标题被本地改名过（`session_marks.title_override` 非空）：界面画铅笔小标。
    pub renamed: bool,
    /// 正文搜索的命中摘要（`Filter.body` 命中时才有值；标题搜索为 null）。
    pub snippet: Option<String>,
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
