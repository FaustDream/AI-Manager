//! 定时任务：把「什么时候自动跑一次签到 / 积分刷新」交给用户配，而不是写死在启动代码里。
//!
//! 存 kv（`schedules`）而不是新建表：这是一份几十行的配置，不是业务数据，
//! 加表就要加迁移，而迁移的代价远大于它带来的收益。
//!
//! **触发规则只有两种**：每天固定时刻、每隔 N 小时。够用且能被用户一眼看懂；
//! cron 表达式对「给账号签个到」这件事是过度设计。

use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};

const KV_KEY: &str = "schedules";

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
}

impl ScheduleAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::CheckinRound => "签到轮询",
            Self::CreditsRound => "积分刷新",
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
    /// 每隔 `hours` 小时触发一次
    Every,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub action: ScheduleAction,
    pub kind: RuleKind,
    /// `kind = daily` 时的触发时刻，`HH:mm`
    pub at: Option<String>,
    /// `kind = every` 时的间隔小时数
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub hours: Option<u32>,
    pub enabled: bool,
    /// 上次真正跑起来的时刻（RFC3339）。`every` 用它推下一次，也用来判「今天跑过没」
    pub last_run: Option<String>,
}

impl Schedule {
    /// 规则的人话描述，界面与执行记录都用它，避免两处各拼一遍。
    pub fn rule_text(&self) -> String {
        match self.kind {
            RuleKind::Daily => {
                format!("每天 {}", self.at.clone().unwrap_or_else(|| "--:--".into()))
            }
            RuleKind::Every => format!("每 {} 小时", self.hours.unwrap_or(0)),
        }
    }

    /// 下一次该跑的时刻（只用于展示「下次什么时候」）。停用 / 规则不合法一律 None。
    ///
    /// `every` 的锚点是 `last_run`：从没跑过就从「现在」起算，保存后不会立刻触发一次，
    /// 否则用户刚点保存就被跑一轮，会以为是自己点错了。
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
            RuleKind::Every => {
                let hours = self.hours.filter(|h| *h > 0)?;
                Some(self.anchor(now) + Duration::hours(hours as i64))
            }
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

    /// `every` 的起算点：上次跑的时刻，没跑过就是现在。
    fn anchor(&self, now: DateTime<Local>) -> DateTime<Local> {
        self.last_run
            .as_deref()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
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
    fn normalize(mut self) -> Result<Self> {
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
            }
            RuleKind::Every => {
                let hours = self.hours.unwrap_or(0);
                if hours == 0 || hours > 24 * 7 {
                    return Err(AppError::new(
                        ErrorKind::Invalid,
                        "间隔要在 1 小时到 168 小时之间",
                    ));
                }
                self.hours = Some(hours);
                self.at = None;
            }
        }
        if self.id.trim().is_empty() {
            // 时间戳 + 动作：可读、够唯一，不需要引入 uuid 依赖
            self.id = format!(
                "{}-{}",
                self.action.key(),
                Local::now().format("%Y%m%d%H%M%S")
            );
        }
        Ok(self)
    }
}

impl ScheduleAction {
    pub fn key(self) -> &'static str {
        match self {
            Self::CheckinRound => "checkin",
            Self::CreditsRound => "credits",
        }
    }
}

/// 读全部定时任务。第一次读（kv 为空）时写入默认种子，保持升级前的行为不丢。
pub fn list(db: &Db) -> Result<Vec<Schedule>> {
    match db.kv_get(KV_KEY)? {
        Some(text) => {
            // 坏数据不能让设置页整页打不开：退回种子并留一条日志
            match serde_json::from_str::<Vec<Schedule>>(&text) {
                Ok(list) => Ok(list),
                Err(e) => {
                    crate::app::log::log(
                        crate::app::log::Level::Warn,
                        "schedule",
                        format!("定时任务配置解析失败，已重置为默认：{e}"),
                    );
                    let seed = seed();
                    save_all(db, &seed)?;
                    Ok(seed)
                }
            }
        }
        None => {
            let seed = seed();
            save_all(db, &seed)?;
            Ok(seed)
        }
    }
}

/// 默认两条：每天 08:30 签到、每小时刷一次积分。
///
/// 这是升级前写死在 `desktop/mod.rs` 的行为，搬到配置里但默认值不变——
/// 用户不该因为一次升级就「签到突然不跑了」。签到的「每天」带补跑语义，
/// 所以升级后第一次启动仍会像以前一样在启动后不久跑一轮。
fn seed() -> Vec<Schedule> {
    vec![
        Schedule {
            id: "checkin-daily".into(),
            name: "每日签到".into(),
            action: ScheduleAction::CheckinRound,
            kind: RuleKind::Daily,
            at: Some("08:30".into()),
            hours: None,
            enabled: true,
            last_run: None,
        },
        Schedule {
            id: "credits-hourly".into(),
            name: "积分刷新".into(),
            action: ScheduleAction::CreditsRound,
            kind: RuleKind::Every,
            at: None,
            hours: Some(1),
            enabled: true,
            last_run: None,
        },
    ]
}

pub fn save_all(db: &Db, list: &[Schedule]) -> Result<()> {
    let text = serde_json::to_string(list)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("序列化定时任务失败：{e}")))?;
    db.kv_set(KV_KEY, &text)
}

/// 新建或更新一条（按 id 覆盖）。
pub fn save(db: &Db, input: Schedule) -> Result<Schedule> {
    let schedule = input.normalize()?;
    let mut list = list(db)?;
    match list.iter_mut().find(|item| item.id == schedule.id) {
        Some(existing) => {
            // 改规则不该把「上次跑过」一起清掉：`every` 靠它推下一次
            let last_run = existing.last_run.clone();
            *existing = Schedule {
                last_run,
                ..schedule.clone()
            };
        }
        None => list.push(schedule.clone()),
    }
    save_all(db, &list)?;
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
    let mut list = list(db)?;
    let before = list.len();
    list.retain(|item| item.id != id);
    if list.len() == before {
        return Err(AppError::new(ErrorKind::Invalid, "这条定时任务不存在"));
    }
    save_all(db, &list)?;
    crate::app::log::log(
        crate::app::log::Level::Info,
        "schedule",
        format!("定时任务已删除：{id}"),
    );
    Ok(())
}

/// 记一次「已经跑起来了」。**必须在真正执行之前写**：执行可能几十秒，
/// 期间下一跳如果再判一次 due，同一分钟会被触发两次。
pub fn mark_ran(db: &Db, id: &str) -> Result<Option<Schedule>> {
    let mut list = list(db)?;
    let Some(item) = list.iter_mut().find(|item| item.id == id) else {
        return Ok(None);
    };
    item.last_run = Some(Local::now().to_rfc3339());
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
            kind: RuleKind::Daily,
            at: Some(at.into()),
            hours: None,
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
            kind: RuleKind::Every,
            at: None,
            hours: Some(2),
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
        let err = daily("8点半").normalize().unwrap_err();
        assert!(err.message.contains("HH:mm"));

        let mut every = daily("08:30");
        every.kind = RuleKind::Every;
        every.hours = Some(0);
        assert!(every.normalize().is_err(), "0 小时要拒绝");
    }

    /// 新建时补 id、按动作给默认名；daily 会清掉 hours、every 会清掉 at。
    #[test]
    fn normalize_fills_id_and_drops_the_other_field() {
        let mut schedule = daily("08:30");
        schedule.id = String::new();
        schedule.name = "  ".into();
        schedule.hours = Some(5);
        let out = schedule.normalize().unwrap();
        assert!(out.id.starts_with("checkin-"));
        assert_eq!(out.name, "签到轮询");
        assert_eq!(out.hours, None, "daily 不该带 hours");
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
