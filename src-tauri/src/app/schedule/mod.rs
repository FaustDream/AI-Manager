//! 定时任务：把「什么时候自动跑一次签到 / 积分刷新」交给用户配，而不是写死在启动代码里。
//!
//! 存 kv（`schedules`）而不是新建表：这是一份几十行的配置，不是业务数据，
//! 加表就要加迁移，而迁移的代价远大于它带来的收益。
//!
//! **触发规则只有两种**：每天固定时刻、每隔 N 分钟（整小时仍按「每 N 小时」显示）。
//! 够用且能被用户一眼看懂；cron 表达式对「给账号签个到」这件事是过度设计。
//!
//! **定点执行的唯一入口**：所有「每天 HH:mm」都必须走 `RuleKind::Daily`，
//! 由本模块的 `due()` 与 `desktop/mod.rs::spawn_scheduler` 统一消费；
//! 禁止在业务代码里另写 `loop { sleep(..) }` 或独立定时器，避免同一时刻重复执行。

use crate::app::activity::{store as travel_store, TravelPrefs};
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

const KV_KEY: &str = "schedules";
// kv 的单次写入原子，但读改写不是；偏好联动与调度标记必须串行，避免互相覆盖。
static CONFIG_LOCK: Mutex<()> = Mutex::new(());
static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

pub async fn changed() {
    CHANGED.notified().await;
}

/// 配置被外部改写（回滚快照）后唤醒调度循环：不唤醒就要等下一次兜底轮询。
pub fn notify_changed() {
    CHANGED.notify_one();
}

/// 能定时的动作。新增动作 = 这里加一项 + 调度器加一个分支 + 前端 `SCHEDULE_ACTIONS` 加一行。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleAction {
    /// 跑一轮签到（与「手动跑一轮」同一条链路）
    CheckinRound,
    /// 刷一轮余额/配额
    CreditsRound,
    /// 猫猫旅行：回来 → 领取 → 再派出（只处理打开了「自动旅行」的账号）
    TravelRound,
}

impl ScheduleAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::CheckinRound => "签到轮询",
            Self::CreditsRound => "积分刷新",
            Self::TravelRound => "猫猫旅行",
        }
    }
}

/// 触发方式。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// 每天 `at`（`HH:mm`）触发一次
    Daily,
    /// 每隔 `minutes`（分钟，整小时用 `hours`）触发一次
    Every,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub action: ScheduleAction,
    /// None 保留原来的全账号任务；Some 只执行这个账号的旅行。
    #[serde(default)]
    pub account_iid: Option<String>,
    pub kind: RuleKind,
    /// `kind = daily` 时的触发时刻，`HH:mm`
    pub at: Option<String>,
    /// `kind = every` 时的间隔小时数
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub hours: Option<u32>,
    /// `kind = every` 时的间隔分钟数。与 `hours` 二选一，**分钟优先**：
    /// 积分/用量这种要跟得紧的任务按分钟配，签到这种一天一次仍按小时。
    /// 老配置只有 `hours`，读出来照旧走小时，行为不变。
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    #[serde(default)]
    pub minutes: Option<u32>,
    pub enabled: bool,
    /// 上次真正跑起来的时刻（RFC3339）。`every` 用它推下一次，也用来判「今天跑过没」
    pub last_run: Option<String>,
    /// 首次间隔的固定锚点；与真实执行记录分开，升级旧规则时补齐。
    #[serde(default)]
    pub created_at: Option<String>,
}

impl Schedule {
    /// 规则的人话描述，界面与执行记录都用它，避免两处各拼一遍。
    pub fn rule_text(&self) -> String {
        match self.kind {
            RuleKind::Daily => {
                format!("每天 {}", self.at.clone().unwrap_or_else(|| "--:--".into()))
            }
            RuleKind::Every => match self.interval_minutes() {
                // 整小时读成「每 N 小时」更自然（老的 hours 配置正好走这一支）
                Some(m) if m >= 60 && m % 60 == 0 => format!("每 {} 小时", m / 60),
                Some(m) => format!("每 {m} 分钟"),
                None => "每隔 --".into(),
            },
        }
    }

    /// 间隔多少分钟。`minutes` 优先，没有才回落到 `hours`。
    fn interval_minutes(&self) -> Option<i64> {
        match (self.minutes, self.hours) {
            (Some(m), _) if m > 0 => Some(m as i64),
            (_, Some(h)) if h > 0 => Some(h as i64 * 60),
            _ => None,
        }
    }

    /// 下一次该跑的时刻（只用于展示「下次什么时候」）。停用 / 规则不合法一律 None。
    ///
    /// `every` 优先从 `last_run` 起算，首次从持久化的 `created_at` 起算，
    /// 避免每次查询向后推迟首次执行，也不会在刚保存时立即执行。
    pub fn next_run(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        if !self.enabled {
            return None;
        }
        match self.kind {
            RuleKind::Daily => {
                let slot = self.daily_slot(now)?;
                if slot > now {
                    Some(slot)
                } else {
                    // 今天的点已经过了：展示明天同一时刻（今天那次算「已错过」）
                    Local
                        .from_local_datetime(
                            &(now.date_naive() + Duration::days(1)).and_time(slot.time()),
                        )
                        .single()
                }
            }
            RuleKind::Every => Some(self.anchor(now) + Duration::minutes(self.interval_minutes()?)),
        }
    }

    /// 现在该跑吗。
    ///
    /// **每天固定时刻带补跑**：过了点但今天还没跑过，就算 due。这个工具是托盘常驻、
    /// 但不保证 24 小时开机——只在 08:30 那一刻判一次，用户很可能整天都没签上。
    pub fn due(&self, now: DateTime<Local>) -> bool {
        if !self.enabled {
            return false;
        }
        match self.kind {
            RuleKind::Daily => match self.daily_slot(now) {
                Some(slot) => now >= slot && !self.ran_today(now),
                None => false,
            },
            RuleKind::Every => self.next_run(now).map(|at| at <= now).unwrap_or(false),
        }
    }

    /// 今天的触发时刻。
    fn daily_slot(&self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        let at = NaiveTime::parse_from_str(self.at.as_deref()?, "%H:%M").ok()?;
        Local
            .from_local_datetime(&now.date_naive().and_time(at))
            .single()
    }

    fn anchor(&self, now: DateTime<Local>) -> DateTime<Local> {
        self.last_run
            .as_deref()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .or_else(|| {
                self.created_at
                    .as_deref()
                    .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            })
            .map(|at| at.with_timezone(&Local))
            .unwrap_or(now)
    }

    /// 今天是否已经跑过（按本地日期比）。
    fn ran_today(&self, now: DateTime<Local>) -> bool {
        self.last_run
            .as_deref()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|at| at.with_timezone(&Local).date_naive() == now.date_naive())
            .unwrap_or(false)
    }

    /// 校验并补全。id 为空表示新建，由这里分配。
    fn normalize(mut self, now: DateTime<Local>) -> Result<Self> {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            self.name = self.action.label().to_string();
        }
        match self.kind {
            RuleKind::Daily => {
                let at = self.at.as_deref().unwrap_or("").trim().to_string();
                NaiveTime::parse_from_str(&at, "%H:%M").map_err(|_| {
                    AppError::new(ErrorKind::Invalid, "时刻要写成 HH:mm，例如 08:30")
                })?;
                self.at = Some(at);
                self.hours = None;
                self.minutes = None;
            }
            RuleKind::Every => {
                // 老界面只传 hours、新界面传 minutes：先统一折算成分钟再校验。
                // 存回时整小时仍补 hours，旧界面/旧配置读到的口径不变。
                let minutes = match (
                    self.minutes.filter(|m| *m > 0),
                    self.hours.filter(|h| *h > 0),
                ) {
                    (Some(minutes), _) => minutes,
                    (None, Some(hours)) => hours.saturating_mul(60),
                    (None, None) => 0,
                };
                if minutes == 0 || minutes > 24 * 60 * 7 {
                    return Err(AppError::new(
                        ErrorKind::Invalid,
                        "间隔要在 1 分钟到 10080 分钟（7 天）之间",
                    ));
                }
                self.minutes = Some(minutes);
                self.hours = if minutes % 60 == 0 {
                    Some(minutes / 60)
                } else {
                    None
                };
                self.at = None;
            }
        }
        if self.id.trim().is_empty() {
            // 时间戳 + 动作：可读、够唯一，不需要引入 uuid 依赖
            self.id = format!("{}-{}", self.action.key(), now.format("%Y%m%d%H%M%S"));
        }
        if self.created_at.is_none() {
            self.created_at = Some(now.to_rfc3339());
        }
        if self.account_iid.is_some() && self.action != ScheduleAction::TravelRound {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "账号关联任务只能用于猫猫旅行",
            ));
        }
        Ok(self)
    }
}

impl ScheduleAction {
    pub fn key(self) -> &'static str {
        match self {
            Self::CheckinRound => "checkin",
            Self::CreditsRound => "credits",
            Self::TravelRound => "travel",
        }
    }
}

/// 默认积分刷新间隔（分钟）。改这里就等于改「新装用户多久看到一次新余额」。
///
/// 取 5 分钟而不是 10：余额变化本身是几分钟级的（签到/旅行到账），
/// 而一轮刷新只有十几个轻量 GET（并发 3），五分钟完全扛得住。
/// 用户嫌吵或撞上限流，在设置里改回小时即可。
const SEED_CREDITS_MINUTES: u32 = 5;

pub fn list(db: &Db) -> Result<Vec<Schedule>> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(db, Local::now())
}

fn load(db: &Db, now: DateTime<Local>) -> Result<Vec<Schedule>> {
    let mut dirty = false;
    let mut list = match db.kv_get(KV_KEY)? {
        Some(text) => match serde_json::from_str::<Vec<Schedule>>(&text) {
            Ok(list) => list,
            Err(e) => {
                crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "schedule",
                    format!("定时任务配置解析失败，已重置为默认：{e}"),
                );
                dirty = true;
                seed()
            }
        },
        None => {
            dirty = true;
            seed()
        }
    };
    for task in &mut list {
        if task.created_at.is_none() {
            task.created_at = Some(now.to_rfc3339());
            dirty = true;
        }
        if tighten_legacy_credits(task) {
            dirty = true;
        }
    }
    let prefs = travel_store::all(db);
    if prefs.values().any(|p| p.enabled) {
        for account in crate::app::accounts::repo::list(db)? {
            if prefs.get(&account.iid).is_some_and(|p| p.enabled)
                && crate::app::providers::for_iid(&account.iid)
                    .is_some_and(|p| p.travel_supported())
                && !list
                    .iter()
                    .any(|s| s.account_iid.as_deref() == Some(&account.iid))
            {
                list.push(travel_task(&account.iid, &account.display_name(), now));
                dirty = true;
            }
        }
    }
    if dirty {
        save_all(db, &list)?;
        CHANGED.notify_one();
    }
    Ok(list)
}

/// 旧版「积分刷新」按小时（默认 1 小时）配下的任务，读到就收紧到 `SEED_CREDITS_MINUTES`。
///
/// 判定必须窄：只认**默认种子那两个 id**（`credits-hourly` / `credits-10min`），
/// 且必须是全局、`every`、启用、间隔正好 60 分钟。用户自己改过的 id 与间隔一律不动——
/// 他能改成 3 小时，说明已经知道这里可配，再替他改就是抢方向盘。
fn tighten_legacy_credits(task: &mut Schedule) -> bool {
    let legacy = task.action == ScheduleAction::CreditsRound
        && task.account_iid.is_none()
        && task.kind == RuleKind::Every
        && task.enabled
        && matches!(task.id.as_str(), "credits-hourly" | "credits-10min")
        && task.minutes.is_none()
        && task.hours == Some(1);
    if !legacy {
        return false;
    }
    task.minutes = Some(SEED_CREDITS_MINUTES);
    task.hours = None;
    task.id = format!("credits-{SEED_CREDITS_MINUTES}min");
    true
}

fn travel_task(iid: &str, name: &str, now: DateTime<Local>) -> Schedule {
    Schedule {
        id: format!("travel-account:{iid}"),
        name: format!("猫猫旅行 · {name}"),
        action: ScheduleAction::TravelRound,
        account_iid: Some(iid.into()),
        kind: RuleKind::Every,
        at: None,
        hours: Some(1),
        minutes: None,
        enabled: true,
        last_run: None,
        created_at: Some(now.to_rfc3339()),
    }
}

/// 偏好与规则共用同一个读改写锁；开关只改 enabled，不重置用户时间或执行记录。
pub fn sync_travel_prefs(
    db: &Db,
    iid: &str,
    name: &str,
    prefs: TravelPrefs,
) -> Result<TravelPrefs> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = Local::now();
    let mut list = load(db, now)?;
    if let Some(task) = list
        .iter_mut()
        .find(|s| s.account_iid.as_deref() == Some(iid))
    {
        task.enabled = prefs.enabled;
    } else if prefs.enabled {
        list.push(travel_task(iid, name, now));
    }
    let saved = travel_store::set(db, iid, prefs)?;
    save_all(db, &list)?;
    CHANGED.notify_one();
    Ok(saved)
}

/// 默认两条：每天 08:30 签到、每 `SEED_CREDITS_MINUTES` 分钟刷一次积分。
///
/// 积分默认给到分钟级：一小时的间隔让列表里的余额看起来永远是旧的，
/// 而积分接口本身很轻（一轮十几个账号两秒多），五分钟完全扛得住。
/// 用户嫌吵或撞上限流，在设置里改回小时即可。
///
/// 签到的默认值沿用升级前写死在 `desktop/mod.rs` 的行为——用户不该因为一次
/// 升级就「签到突然不跑了」；「每天」带补跑语义，升级后首次启动仍会补跑一轮。
fn seed() -> Vec<Schedule> {
    vec![
        Schedule {
            id: "checkin-daily".into(),
            name: "每日签到".into(),
            action: ScheduleAction::CheckinRound,
            account_iid: None,
            created_at: None,
            kind: RuleKind::Daily,
            at: Some("08:30".into()),
            hours: None,
            minutes: None,
            enabled: true,
            last_run: None,
        },
        Schedule {
            id: format!("credits-{SEED_CREDITS_MINUTES}min"),
            name: "积分刷新".into(),
            action: ScheduleAction::CreditsRound,
            account_iid: None,
            created_at: None,
            kind: RuleKind::Every,
            at: None,
            hours: None,
            minutes: Some(SEED_CREDITS_MINUTES),
            enabled: true,
            last_run: None,
        },
    ]
}

fn save_all(db: &Db, list: &[Schedule]) -> Result<()> {
    let text = serde_json::to_string(list)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("序列化定时任务失败：{e}")))?;
    // 写前留一份：定时任务改错了只能靠这个回滚（设置页「配置历史」）
    crate::app::config_history::capture(db, KV_KEY, Some(&text))?;
    db.kv_set(KV_KEY, &text)
}

/// 新建或更新一条（按 id 覆盖）。
pub fn save(db: &Db, input: Schedule) -> Result<Schedule> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = Local::now();
    let mut list = load(db, now)?;
    let mut input = input;
    if let Some(existing) = list.iter().find(|s| s.id == input.id) {
        // 旧界面可能不回传关联字段，但不允许把账号任务改成另一账号或全局任务。
        if input.account_iid.is_some() && input.account_iid != existing.account_iid {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "不能修改定时任务关联的账号",
            ));
        }
        input.account_iid = existing.account_iid.clone();
        input.last_run = existing.last_run.clone();
        input.created_at = existing.created_at.clone();
    } else {
        input.last_run = None;
        input.created_at = Some(now.to_rfc3339());
        if let Some(iid) = &input.account_iid {
            if crate::app::accounts::repo::identity(db, iid)?.is_none()
                || !crate::app::providers::for_iid(iid).is_some_and(|p| p.travel_supported())
            {
                return Err(AppError::new(ErrorKind::Invalid, "这个账号不支持猫猫旅行"));
            }
            input.id = format!("travel-account:{iid}");
            if list
                .iter()
                .any(|s| s.account_iid.as_deref() == Some(iid) || s.id == input.id)
            {
                return Err(AppError::new(
                    ErrorKind::Invalid,
                    "这个账号已有旅行定时任务",
                ));
            }
        }
    }
    let schedule = input.normalize(now)?;
    match list.iter_mut().find(|item| item.id == schedule.id) {
        Some(existing) => *existing = schedule.clone(),
        None => list.push(schedule.clone()),
    }
    save_all(db, &list)?;
    CHANGED.notify_one();
    crate::app::log::log(
        crate::app::log::Level::Info,
        "schedule",
        format!(
            "定时任务已保存：{} · {}",
            schedule.name,
            schedule.rule_text()
        ),
    );
    Ok(schedule)
}

pub fn delete(db: &Db, id: &str) -> Result<()> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = load(db, Local::now())?;
    let Some(index) = list.iter().position(|item| item.id == id) else {
        return Err(AppError::new(ErrorKind::Invalid, "这条定时任务不存在"));
    };
    let removed = list.remove(index);
    if let Some(iid) = &removed.account_iid {
        // 明确删除意味着退出自动旅行，否则下次读列表会被旧偏好重新补回。
        let mut prefs = travel_store::prefs_of(db, iid);
        prefs.enabled = false;
        travel_store::set(db, iid, prefs)?;
    }
    save_all(db, &list)?;
    CHANGED.notify_one();
    crate::app::log::log(
        crate::app::log::Level::Info,
        "schedule",
        format!("定时任务已删除：{}", removed.name),
    );
    Ok(())
}

/// 记一次「已经跑起来了」。**必须在真正执行之前写**：执行可能几十秒，
/// 期间下一跳如果再判一次 due，同一分钟会被触发两次。
pub fn mark_ran(db: &Db, id: &str) -> Result<Option<Schedule>> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let result = mark(db, id, Local::now(), false)?;
    CHANGED.notify_one();
    Ok(result)
}

/// 不能执行睡眠前的快照：在同一读改写锁内重新检查开关、时间并记执行时刻。
pub fn take_due(db: &Db, id: &str, now: DateTime<Local>) -> Result<Option<Schedule>> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    mark(db, id, now, true)
}

fn mark(db: &Db, id: &str, now: DateTime<Local>, only_due: bool) -> Result<Option<Schedule>> {
    let mut list = load(db, now)?;
    let Some(item) = list.iter_mut().find(|item| item.id == id) else {
        return Ok(None);
    };
    if only_due && !item.due(now) {
        return Ok(None);
    }
    item.last_run = Some(now.to_rfc3339());
    let snapshot = item.clone();
    save_all(db, &list)?;
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn daily(at: &str) -> Schedule {
        Schedule {
            id: "d".into(),
            name: "签到".into(),
            action: ScheduleAction::CheckinRound,
            account_iid: None,
            created_at: None,
            kind: RuleKind::Daily,
            at: Some(at.into()),
            hours: None,
            minutes: None,
            enabled: true,
            last_run: None,
        }
    }

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    /// 每天固定时刻：今天没过就是今天，过了就是明天。
    #[test]
    fn daily_next_run_rolls_to_tomorrow() {
        let now = at(2026, 11, 4, 7, 0);
        let next = daily("08:30").next_run(now).unwrap();
        assert_eq!(next, at(2026, 11, 4, 8, 30));

        let now = at(2026, 11, 4, 9, 0);
        let next = daily("08:30").next_run(now).unwrap();
        assert_eq!(next, at(2026, 11, 5, 8, 30), "过了点要顺延到明天");
    }

    /// 每隔 N 小时：锚点是 last_run；从没跑过就从现在起算。
    #[test]
    fn every_uses_last_run_as_anchor() {
        let mut schedule = Schedule {
            id: "e".into(),
            name: "积分".into(),
            action: ScheduleAction::CreditsRound,
            account_iid: None,
            created_at: None,
            kind: RuleKind::Every,
            at: None,
            hours: Some(2),
            minutes: None,
            enabled: true,
            last_run: None,
        };
        let now = at(2026, 11, 4, 10, 0);
        assert_eq!(schedule.next_run(now).unwrap(), at(2026, 11, 4, 12, 0));
        assert!(!schedule.due(now), "刚保存不能立刻触发");

        schedule.last_run = Some(at(2026, 11, 4, 9, 0).to_rfc3339());
        assert_eq!(schedule.next_run(now).unwrap(), at(2026, 11, 4, 11, 0));
        assert!(schedule.due(at(2026, 11, 4, 11, 0)), "到点就要跑");
    }

    /// 分钟级间隔：积分要跟得紧，interval 直接按分钟算，不被小时粒度卡住。
    #[test]
    fn every_supports_minute_intervals() {
        let mut schedule = Schedule {
            id: "m".into(),
            name: "积分".into(),
            action: ScheduleAction::CreditsRound,
            account_iid: None,
            created_at: None,
            kind: RuleKind::Every,
            at: None,
            hours: None,
            minutes: Some(10),
            enabled: true,
            last_run: None,
        };
        let now = at(2026, 11, 4, 10, 0);
        assert_eq!(schedule.next_run(now).unwrap(), at(2026, 11, 4, 10, 10));
        assert_eq!(schedule.rule_text(), "每 10 分钟");

        schedule.last_run = Some(at(2026, 11, 4, 10, 0).to_rfc3339());
        assert!(schedule.due(at(2026, 11, 4, 10, 10)));
        assert!(!schedule.due(at(2026, 11, 4, 10, 9)), "差一分钟不该跑");

        // 整小时仍读成「每 N 小时」，老的 hours 配置和新的 minutes=60 是同一件事
        schedule.minutes = Some(120);
        assert_eq!(schedule.rule_text(), "每 2 小时");
    }

    /// 老界面只传 hours：normalize 折算成 minutes 落库，同时把 hours 补回来。
    #[test]
    fn normalize_bridges_hours_and_minutes() {
        let mut by_hours = daily("08:30");
        by_hours.kind = RuleKind::Every;
        by_hours.hours = Some(2);
        let out = by_hours.normalize(at(2026, 11, 4, 10, 0)).unwrap();
        assert_eq!(out.minutes, Some(120));
        assert_eq!(out.hours, Some(2), "整小时要回填，旧界面读到的口径不变");

        let mut by_minutes = daily("08:30");
        by_minutes.kind = RuleKind::Every;
        by_minutes.minutes = Some(7);
        let out = by_minutes.normalize(at(2026, 11, 4, 10, 0)).unwrap();
        assert_eq!(out.minutes, Some(7));
        assert_eq!(out.hours, None, "不是整小时就不编一个 hours 出来");
    }

    /// 每天固定时刻：错过要点就补跑，但今天跑过就不再跑。
    #[test]
    fn daily_catches_up_once_per_day() {
        let now = at(2026, 11, 4, 21, 0);
        let mut schedule = daily("08:30");
        assert!(schedule.due(now), "过了点且今天没跑过 → 补跑");

        schedule.last_run = Some(at(2026, 11, 4, 8, 31).to_rfc3339());
        assert!(!schedule.due(now), "今天已经跑过，不再补");

        // 昨天的记录不影响今天补跑
        schedule.last_run = Some(at(2026, 11, 3, 8, 31).to_rfc3339());
        assert!(schedule.due(now));
        assert_eq!(
            schedule.next_run(now).unwrap(),
            at(2026, 11, 5, 8, 30),
            "展示用的「下次」仍是明天那个点"
        );
    }

    /// 停用的任务永远不 due。
    #[test]
    fn disabled_is_never_due() {
        let mut schedule = daily("00:00");
        schedule.enabled = false;
        assert!(schedule.next_run(at(2026, 11, 4, 12, 0)).is_none());
        assert!(!schedule.due(at(2026, 11, 4, 12, 0)));
    }

    #[test]
    fn normalize_rejects_bad_rules() {
        let err = daily("8点半")
            .normalize(at(2026, 11, 4, 10, 0))
            .unwrap_err();
        assert!(err.message.contains("HH:mm"));

        let mut every = daily("08:30");
        every.kind = RuleKind::Every;
        every.hours = Some(0);
        assert!(
            every.normalize(at(2026, 11, 4, 10, 0)).is_err(),
            "0 小时要拒绝"
        );
    }

    /// 新建时补 id、按动作给默认名；daily 会清掉 hours、every 会清掉 at。
    #[test]
    fn normalize_fills_id_and_drops_the_other_field() {
        let mut schedule = daily("08:30");
        schedule.id = String::new();
        schedule.name = "  ".into();
        schedule.hours = Some(5);
        let out = schedule.normalize(at(2026, 11, 4, 10, 0)).unwrap();
        assert!(out.id.starts_with("checkin-"));
        assert_eq!(out.name, "签到轮询");
        assert_eq!(out.hours, None, "daily 不该带 hours");
    }

    fn add_account(db: &Db, iid: &str, name: &str) {
        crate::app::accounts::repo::upsert_identity(
            db,
            &crate::app::accounts::view::AccountIdentity {
                iid: iid.into(),
                platform: iid.split(':').next().unwrap().into(),
                platform_label: "测试平台".into(),
                account_id: iid.into(),
                name: name.into(),
                ..Default::default()
            },
            0,
        )
        .unwrap();
    }

    fn travel_tasks(db: &Db) -> Vec<Schedule> {
        list(db)
            .unwrap()
            .into_iter()
            .filter(|s| s.action == ScheduleAction::TravelRound)
            .collect()
    }

    #[tokio::test]
    async fn travel_prefs_create_and_toggle_one_linked_task() {
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());
        add_account(&db, "workbuddy:u1", "猫猫账号一");
        let service = crate::app::activity::Service::new(
            db.clone(),
            std::sync::Arc::new(crate::app::net::MockHttp::new()),
        );
        service
            .set_prefs("workbuddy:u1", true, Some(1))
            .await
            .unwrap();
        let tasks = travel_tasks(&db);
        assert_eq!(tasks.len(), 1, "开启自动旅行必须创建账号定时任务");
        assert!(tasks[0].enabled);
        assert_eq!(tasks[0].hours, Some(1));
        assert!(tasks[0].name.contains("猫猫账号一"));
        assert_eq!(
            serde_json::to_value(&tasks[0]).unwrap()["account_iid"],
            "workbuddy:u1"
        );
        let id = tasks[0].id.clone();
        service
            .set_prefs("workbuddy:u1", false, Some(1))
            .await
            .unwrap();
        let tasks = travel_tasks(&db);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, id);
        assert!(!tasks[0].enabled);
        service
            .set_prefs("workbuddy:u1", true, Some(2))
            .await
            .unwrap();
        let tasks = travel_tasks(&db);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, id);
        assert!(tasks[0].enabled);
    }

    #[tokio::test]
    async fn travel_toggle_preserves_custom_rule_and_last_run() {
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());
        add_account(&db, "workbuddy:u1", "猫猫账号一");
        let service = crate::app::activity::Service::new(
            db.clone(),
            std::sync::Arc::new(crate::app::net::MockHttp::new()),
        );
        service.set_prefs("workbuddy:u1", true, None).await.unwrap();
        let mut task = travel_tasks(&db).pop().expect("应创建旅行定时任务");
        task.kind = RuleKind::Daily;
        task.at = Some("21:15".into());
        task.name = "自定任务名".into();
        save(&db, task.clone()).unwrap();
        let marked = mark_ran(&db, &task.id).unwrap().unwrap();
        service
            .set_prefs("workbuddy:u1", false, None)
            .await
            .unwrap();
        service.set_prefs("workbuddy:u1", true, None).await.unwrap();
        let stored = travel_tasks(&db).pop().unwrap();
        assert_eq!(stored.kind, RuleKind::Daily);
        assert_eq!(stored.at.as_deref(), Some("21:15"));
        assert_eq!(stored.hours, None);
        assert_eq!(stored.name, "自定任务名");
        assert_eq!(stored.last_run, marked.last_run);
    }

    #[tokio::test]
    async fn travel_accounts_have_independent_schedules() {
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());
        add_account(&db, "workbuddy:u1", "账号一");
        add_account(&db, "workbuddy:u2", "账号二");
        let service = crate::app::activity::Service::new(
            db.clone(),
            std::sync::Arc::new(crate::app::net::MockHttp::new()),
        );
        let (first, second) = tokio::join!(
            service.set_prefs("workbuddy:u1", true, None),
            service.set_prefs("workbuddy:u2", true, None),
        );
        first.unwrap();
        second.unwrap();
        let tasks = travel_tasks(&db);
        assert_eq!(tasks.len(), 2);
        assert_ne!(tasks[0].id, tasks[1].id);
        let second = tasks
            .iter()
            .find(|s| s.name.contains("账号二"))
            .unwrap()
            .clone();
        service
            .set_prefs("workbuddy:u1", false, None)
            .await
            .unwrap();
        assert!(
            travel_tasks(&db).contains(&second),
            "关闭账号一不得改动账号二"
        );
    }

    #[test]
    fn travel_legacy_enabled_prefs_are_backfilled_without_overwriting_edits() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        add_account(&db, "workbuddy:u1", "账号一");
        add_account(&db, "workbuddy:u2", "账号二");
        crate::app::activity::store::set(
            &db,
            "workbuddy:u1",
            crate::app::activity::TravelPrefs {
                enabled: true,
                location_id: None,
            },
        )
        .unwrap();
        let mut task = travel_tasks(&db).pop().expect("旧版已开启偏好应补齐任务");
        task.enabled = false;
        task.hours = Some(6);
        // 落库前 normalize 会把小时折算成分钟并一起存：预期值要跟它对齐，
        // 否则断言比的是「未归一化的输入」而不是「读回来的配置」
        task.minutes = Some(360);
        save(&db, task.clone()).unwrap();
        assert_eq!(travel_tasks(&db), vec![task.clone()]);
        assert_eq!(
            travel_tasks(&db),
            vec![task],
            "反复读取不能覆盖用户停用与自定义时间"
        );
    }

    #[test]
    fn every_first_deadline_does_not_drift() {
        let created = at(2026, 11, 4, 10, 0);
        let mut value = serde_json::to_value(daily("08:30")).unwrap();
        value["kind"] = "every".into();
        value["at"] = serde_json::Value::Null;
        value["hours"] = 1.into();
        value["created_at"] = created.to_rfc3339().into();
        let schedule: Schedule = serde_json::from_value(value).unwrap();
        assert_eq!(schedule.last_run, None, "创建不是执行");
        assert!(!schedule.due(created + Duration::minutes(59)));
        assert!(
            schedule.due(created + Duration::hours(1)),
            "首次间隔到点必须触发"
        );
        assert_eq!(
            schedule.next_run(created + Duration::minutes(30)),
            Some(created + Duration::hours(1))
        );
    }

    #[tokio::test]
    async fn travel_prefs_reject_unsupported_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());
        add_account(&db, "qoder:u1", "非旅行平台");
        let service = crate::app::activity::Service::new(
            db.clone(),
            std::sync::Arc::new(crate::app::net::MockHttp::new()),
        );
        assert!(service.set_prefs("qoder:u1", true, None).await.is_err());
        assert!(!crate::app::activity::store::prefs_of(&db, "qoder:u1").enabled);
    }

    #[test]
    fn every_legacy_anchor_is_persisted_without_faking_a_run() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let mut legacy = serde_json::to_value(daily("08:30")).unwrap();
        legacy.as_object_mut().unwrap().remove("created_at");
        legacy.as_object_mut().unwrap().remove("account_iid");
        legacy["kind"] = "every".into();
        legacy["hours"] = 1.into();
        db.kv_set(KV_KEY, &serde_json::json!([legacy]).to_string())
            .unwrap();
        let now = at(2026, 11, 4, 10, 0);
        let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let first = load(&db, now).unwrap().remove(0);
        let later = load(&db, now + Duration::minutes(30)).unwrap().remove(0);
        assert_eq!(first, later);
        assert_eq!(later.created_at, Some(now.to_rfc3339()));
        assert_eq!(later.last_run, None);
        assert_eq!(later.account_iid, None);
        assert!(later.due(now + Duration::hours(1)));
    }

    #[test]
    fn every_saved_task_and_seed_have_a_fixed_first_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let seed = list(&db)
            .unwrap()
            .into_iter()
            .find(|s| s.kind == RuleKind::Every)
            .unwrap();
        let mut input = daily("08:30");
        input.kind = RuleKind::Every;
        input.hours = Some(1);
        let saved = save(&db, input).unwrap();
        for task in [seed, saved] {
            let created = DateTime::parse_from_rfc3339(task.created_at.as_deref().unwrap())
                .unwrap()
                .with_timezone(&Local);
            assert_eq!(task.last_run, None);
            assert!(!task.due(created));
            assert!(task.due(created + Duration::hours(1)));
        }
    }

    #[test]
    fn take_due_rechecks_disabled_rescheduled_and_deleted_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let mut task = save(&db, daily("00:00")).unwrap();
        let now = at(2026, 11, 4, 12, 0);
        assert!(task.due(now));
        task.enabled = false;
        save(&db, task.clone()).unwrap();
        assert!(take_due(&db, &task.id, now).unwrap().is_none());
        task.enabled = true;
        task.at = Some("23:30".into());
        save(&db, task.clone()).unwrap();
        assert!(take_due(&db, &task.id, now).unwrap().is_none());
        task.at = Some("00:00".into());
        save(&db, task.clone()).unwrap();
        let fresh = take_due(&db, &task.id, now).unwrap().unwrap();
        assert_eq!(fresh.last_run, Some(now.to_rfc3339()));
        assert!(
            take_due(&db, &task.id, now).unwrap().is_none(),
            "同一轮不得重复取出"
        );
        delete(&db, &task.id).unwrap();
        assert!(take_due(&db, &task.id, now).unwrap().is_none());
    }

    #[tokio::test]
    async fn travel_round_for_isolates_accounts_and_keeps_safety_gates() {
        use crate::app::accounts::repo;
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());
        let http = std::sync::Arc::new(crate::app::net::MockHttp::new());
        let status_url = "https://www.workbuddy.cn/activity/growth/buddy/travel/status";
        http.on(status_url, 200, r#"{"code":0,"data":{"state":"limit"}}"#);
        let service = crate::app::activity::Service::new(db.clone(), http.clone());
        for iid in ["workbuddy:u1", "workbuddy:u2", "workbuddy:no-secret"] {
            add_account(&db, iid, iid);
            service.set_prefs(iid, true, None).await.unwrap();
            if iid != "workbuddy:no-secret" {
                let mut cred = crate::app::accounts::credential::Credential::default();
                cred.put("uid", iid);
                repo::set_secret(&db, iid, &cred.to_bytes().unwrap(), None).unwrap();
            }
        }
        let round = service.round_for("test", Some("workbuddy:u1")).await;
        assert_eq!(round.total, 1);
        assert_eq!(round.failed, 0);
        assert_eq!(round.waiting, 1);
        assert_eq!(http.call_count(status_url), 1);
        assert_eq!(
            http.last_header(status_url, "X-User-Id").as_deref(),
            Some("workbuddy:u1")
        );
        assert_eq!(
            service
                .round_for("test", Some("workbuddy:no-secret"))
                .await
                .total,
            0
        );
        repo::set_enabled(&db, "workbuddy:u1", false).unwrap();
        assert_eq!(
            service.round_for("test", Some("workbuddy:u1")).await.total,
            0
        );
        repo::set_enabled(&db, "workbuddy:u1", true).unwrap();
        service
            .set_prefs("workbuddy:u1", false, None)
            .await
            .unwrap();
        assert_eq!(
            service.round_for("test", Some("workbuddy:u1")).await.total,
            0
        );
        assert_eq!(
            service
                .round_for("test", Some("workbuddy:missing"))
                .await
                .total,
            0
        );
        assert_eq!(
            http.call_count(status_url),
            1,
            "不符合条件的账号不得发起请求"
        );
        service.set_prefs("workbuddy:u1", true, None).await.unwrap();
        assert_eq!(
            service.round("test").await.total,
            2,
            "全局入口仍处理全部符合条件账号"
        );
        assert_eq!(
            http.calls(),
            vec![status_url.to_string(); 3],
            "不得调用领取或派出接口"
        );
    }

    /// 实测形态：`state=arrived`、`reward_credit=9`、`daily_limit_reached=true`。
    ///
    /// 上限约束的是「今天还能不能再派一趟」，不是「已到手的奖励不许领」。
    /// 先判上限会让奖励永远卡在待领取：界面上看得到「已回来」，日志里什么都没有。
    #[tokio::test]
    async fn arrived_claims_even_when_daily_limit_is_reached() {
        use crate::app::accounts::repo;
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());
        let http = std::sync::Arc::new(crate::app::net::MockHttp::new());
        let status_url = "https://www.workbuddy.cn/activity/growth/buddy/travel/status";
        let claim_url = "https://www.workbuddy.cn/activity/growth/buddy/travel/claim";
        let depart_url = "https://www.workbuddy.cn/activity/growth/buddy/travel/depart";
        http.on(
            status_url,
            200,
            r#"{"code":0,"data":{"state":"arrived","daily_limit_reached":true,"reward_credit":9}}"#,
        );
        http.on(
            claim_url,
            200,
            r#"{"code":0,"msg":"OK","data":{"reward_credit":9}}"#,
        );

        let service = crate::app::activity::Service::new(db.clone(), http.clone());
        add_account(&db, "workbuddy:u1", "u1");
        service.set_prefs("workbuddy:u1", true, None).await.unwrap();
        let mut cred = crate::app::accounts::credential::Credential::default();
        cred.put("uid", "u1");
        repo::set_secret(&db, "workbuddy:u1", &cred.to_bytes().unwrap(), None).unwrap();

        let round = service.round_for("test", Some("workbuddy:u1")).await;
        assert_eq!(round.claimed, 1, "到达状态必须领取，即便今天已不能再派出");
        assert_eq!(http.call_count(claim_url), 1);
        assert_eq!(
            http.call_count(depart_url),
            0,
            "上限已到，不该再白发一次派出请求"
        );
    }

    #[tokio::test]
    async fn travel_explicit_delete_does_not_resurrect_on_list() {
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Db::open_at(&dir.path().join("app.db")).unwrap());
        add_account(&db, "workbuddy:u1", "账号一");
        let service = crate::app::activity::Service::new(
            db.clone(),
            std::sync::Arc::new(crate::app::net::MockHttp::new()),
        );
        service
            .set_prefs("workbuddy:u1", true, Some(3))
            .await
            .unwrap();
        let task = travel_tasks(&db).pop().unwrap();
        delete(&db, &task.id).unwrap();
        assert!(travel_tasks(&db).is_empty());
        let prefs = travel_store::prefs_of(&db, "workbuddy:u1");
        assert!(!prefs.enabled);
        assert_eq!(prefs.location_id, Some(3));
    }

    /// 老库里默认那条「每小时刷积分」读到就收紧到分钟级——否则改代码对它无效。
    /// 反之：用户自己配的间隔、关掉的任务、别的任务一律不动。
    #[test]
    fn legacy_hourly_credits_seed_is_tightened_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let legacy = serde_json::to_value(Schedule {
            id: "credits-hourly".into(),
            name: "积分刷新".into(),
            action: ScheduleAction::CreditsRound,
            account_iid: None,
            created_at: None,
            kind: RuleKind::Every,
            at: None,
            hours: Some(1),
            minutes: None,
            enabled: true,
            last_run: None,
        })
        .unwrap();
        // 用户自己改过的同类任务：3 小时、自定 id，必须原样保留
        let mut custom = legacy.clone();
        custom["id"] = "credits-mine".into();
        custom["hours"] = 3.into();
        // 用户停用的默认任务：不擅自恢复成更快的频率
        let mut disabled = legacy.clone();
        disabled["id"] = "credits-off".into();
        disabled["enabled"] = false.into();
        db.kv_set(
            KV_KEY,
            &serde_json::json!([legacy, custom, disabled]).to_string(),
        )
        .unwrap();

        let tasks = list(&db).unwrap();
        let tightened = tasks.iter().find(|s| s.minutes.is_some()).unwrap();
        assert_eq!(tightened.id, format!("credits-{SEED_CREDITS_MINUTES}min"));
        assert_eq!(tightened.minutes, Some(SEED_CREDITS_MINUTES));
        assert_eq!(tightened.hours, None);
        let custom = tasks.iter().find(|s| s.id == "credits-mine").unwrap();
        assert_eq!(custom.hours, Some(3));
        assert_eq!(custom.minutes, None);
        let off = tasks.iter().find(|s| s.id == "credits-off").unwrap();
        assert_eq!(off.hours, Some(1));
        assert!(!off.enabled);
    }

    #[test]
    fn store_roundtrip_and_seed() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        // 第一次读：写入默认种子
        let seeded = list(&db).unwrap();
        assert_eq!(seeded.len(), 2);
        assert!(seeded
            .iter()
            .any(|s| s.action == ScheduleAction::CheckinRound));

        let mut added = daily("21:15");
        added.id = String::new();
        let saved = save(&db, added).unwrap();
        assert_eq!(list(&db).unwrap().len(), 3);

        let marked = mark_ran(&db, &saved.id).unwrap().unwrap();
        assert!(marked.last_run.is_some(), "跑过要留痕");

        delete(&db, &saved.id).unwrap();
        assert_eq!(list(&db).unwrap().len(), 2);
        assert!(delete(&db, "nope").is_err());
    }
}
