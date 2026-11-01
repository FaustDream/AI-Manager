//! 各阶段要实现的契约接口。**本模块只定签名，不含实现。**
//!
//! 存在的理由：P0 把接口钉死，P2~P6 逐个实现。签名错了会在后期放大，
//! 所以这里每一条都对应 ARCHITECTURE-v2.md §7 的一个契约编号，改动需人工审。

use crate::app::clock::Clock;
use crate::app::error::Result;
use crate::types::{PlatformKey, SessionRow};
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};

/// 目标平台（技能与 MCP 的落点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TargetKey {
    QoderCn,
    Qoder,
    TraeCn,
    Trae,
    WorkBuddy,
    WorkBuddyIntl,
    CodeBuddy,
    ZCode,
}

/// 统一 MCP 条目（内部一律用 disabled 表示停用）。
#[derive(Debug, Clone, Default)]
pub struct McpEntry {
    pub name: String,
    pub disabled: bool,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<Vec<(String, String)>>,
    pub timeout_ms: Option<u64>,
}

/// 签到与积分（P3 实现）。契约 1–6、21、22。
pub trait CheckinStore {
    /// 契约 1：接口返回值优先于本地记录，本地只兜底。
    fn resolve_checked(&self, iid: &str, remote: Option<bool>, clock: &dyn Clock) -> bool;
    /// 契约 2：当日去重表。调用接口**前**检查、成功**后**写入、失败不写。
    fn marked_today(&self, iid: &str, clock: &dyn Clock) -> bool;
    fn mark_today(&self, iid: &str, clock: &dyn Clock) -> Result<()>;
    /// 契约 3 + 22：next_at 只由接口下发，且必须能跨重启还原。
    fn next_at(&self, iid: &str) -> Option<DateTime<Local>>;
    fn note_next_at(&self, iid: &str, at: DateTime<Local>) -> Result<()>;
    /// 契约 4：cooldown_hours=0 按自然日，24 按上次 +24h。
    fn next_checkin_at(
        &self,
        cooldown_hours: u32,
        last: DateTime<Local>,
        clock: &dyn Clock,
    ) -> Option<DateTime<Local>>;
    /// 契约 6：同一天多次写入只保留最后一次（upsert 而非 insert）。
    fn note_balance(&self, iid: &str, balance: f64, clock: &dyn Clock) -> Result<bool>;
    fn balance_on(&self, iid: &str, day: &str) -> Option<f64>;
}

/// 积分台账（P3 实现）。契约 5。
pub trait Ledger {
    /// live_credit=true 的平台从不写台账。
    /// `validity_days` 只在 Qoder 这类 live_credit=false 的平台有值。
    fn add_if_needed(
        &self,
        live_credit: bool,
        iid: &str,
        granted: f64,
        validity_days: Option<i64>,
        clock: &dyn Clock,
    ) -> Result<bool>;
    fn count(&self, iid: &str) -> usize;
}

/// 档案往返（P2 实现）。契约 8a——字节一致是「切换不弹登录页」的可自动化代理指标。
pub trait ProfileStore {
    fn save(&self, src: &Path) -> Result<Vec<u8>>;
    fn restore(&self, blob: &[u8], dst: &Path) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    pub changed_rows: usize,
    pub body_files_before: usize,
    pub body_files_after: usize,
    pub backup: PathBuf,
}

/// 会话（P4 实现）。契约 9、10、11。
/// 注意：**本 trait 不含任何跨版本搬运方法**——契约 11 要求不实现，
/// 留在这里是为了让「不存在」这件事能被编译期检查到。
pub trait SessionStore {
    fn page(
        &self,
        cursor: Option<&str>,
        limit: usize,
        state: Option<&str>,
    ) -> Result<crate::types::Page<SessionRow>>;
    /// 契约 10：只改 user_id 引用，正文文件数量与内容都不变。
    fn restore_account(&self, edition: &str, uid: &str) -> Result<RestoreReport>;
}

/// 技能与 MCP（P5 实现）。契约 13–16。
pub trait HubStore {
    /// 契约 13：平台路径常量表。
    fn target_dir(&self, key: TargetKey) -> PathBuf;
    /// 契约 14：统一格式 ↔ 平台格式。
    fn to_platform(&self, entry: &McpEntry, key: TargetKey) -> serde_json::Value;
    // wrong_self_convention：from_* 指的是「从平台格式转回来」，Hub 持有路径
    // 上下文所以带 &self，不是 FromStr 那一族
    #[allow(clippy::wrong_self_convention)]
    fn from_platform(&self, raw: &serde_json::Value, key: TargetKey) -> Result<McpEntry>;
    /// 契约 15：写前备份。
    fn backup_before_write(&self, key: TargetKey) -> Result<PathBuf>;
    /// 契约 16：同名但非本工具写入 → 报冲突，不覆盖。
    fn check_conflict(&self, name: &str, key: TargetKey) -> Result<()>;
}

/// 客户端数据库访问（P2/P4 实现）。契约 9。
pub trait ClientDb {
    /// 只读打开用 SQLITE_OPEN_READ_ONLY；写路径必须先在事务外 checkpoint。
    fn open(&self, path: &Path, write: bool) -> Result<()>;
    fn checkpoint(&self) -> Result<()>;
}

/// 平台能力（P1/P2 实现）。契约 12、20。
pub trait PlatformApi {
    /// 契约 12：单实例互斥体。
    fn acquire_single_instance(&self, name: &str) -> Result<bool>;
    /// 契约 20：外部命令不得弹控制台。
    fn spawn_hidden(&self, exe: &str, args: &[&str]) -> Result<()>;
    fn client_running(&self, key: PlatformKey) -> bool;
}
