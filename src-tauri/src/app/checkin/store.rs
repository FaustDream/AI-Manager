//! 签到相关的四张表 + `kv` 里的 `next_at`。
//!
//! 这里是把 `contracts::CheckinStore` / `contracts::Ledger` 兑现成 SQL 的地方，
//! 每条实现都对着架构文档的一条契约，改动前先看文档再动手。
//!
//! **`next_at` 不落表**：它是状态而不是历史，落在 `kv` 里（键 `next_at:<iid>`），
//! 契约 22 要求重启后还能还原。

use crate::app::accounts::is_intl_account;
use crate::app::clock::Clock;
use crate::app::db::Db;
use crate::app::error::{ErrorKind, Result};
use crate::contracts::{CheckinStore, Ledger};
use chrono::{DateTime, Local, NaiveDate, TimeZone};
use rusqlite::OptionalExtension;

pub const KEEP_CHECKIN_DAYS: i64 = 3;
pub const KEEP_SNAPSHOT_DAYS: i64 = 60;

pub struct SqliteCheckin<'a> {
    db: &'a Db,
}

impl<'a> SqliteCheckin<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    fn next_at_key(iid: &str) -> String {
        format!("next_at:{iid}")
    }

    /// 「今天没签」的结论缓存。`daily_marks` 只记签过的，未签没有落点，
    /// 列表就只能显示「待查询」——早上打开时每个账号都是这个字，等于什么都没说。
    fn unchecked_key(iid: &str) -> String {
        format!("unchecked:{iid}")
    }

    /// 迁移校验：积分快照的 `iid|date` 全集（只读）。
    pub fn ledger_keys(&self) -> Result<Vec<String>> {
        self.db.read(|conn| {
            let mut stmt = conn.prepare("SELECT iid || '|' || date FROM credit_snapshots")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    /// 跨天清理：签到记录留 3 天、余额快照留 60 天、当日去重只留今天。
    /// 启动时跑一次即可；失败只 warn，不影响主流程。
    pub fn cleanup(&self, clock: &dyn Clock) -> Result<usize> {
        let now = clock.now();
        let checkin_before = now.timestamp_millis() - KEEP_CHECKIN_DAYS * 86_400_000;
        let snapshot_before = (now.date_naive() - chrono::Duration::days(KEEP_SNAPSHOT_DAYS))
            .format("%Y-%m-%d")
            .to_string();
        let today = now.date_naive().format("%Y-%m-%d").to_string();
        self.db.write(|conn| {
            let mut removed = 0;
            removed += conn.execute(
                "DELETE FROM checkin_log WHERE checked_at < ?1",
                [checkin_before],
            )?;
            removed += conn.execute(
                "DELETE FROM credit_snapshots WHERE date < ?1",
                [&snapshot_before],
            )?;
            removed += conn.execute("DELETE FROM daily_marks WHERE date <> ?1", [&today])?;
            Ok(removed)
        })
    }

    /// 最近一次本工具亲眼签到的时刻（`checkin_log` 最后一行），冷却推算的起点。
    pub fn last_checkin_at(&self, iid: &str) -> Option<DateTime<Local>> {
        let ms: Option<i64> = self
            .db
            .read(|conn| {
                let value = conn
                    .query_row(
                        "SELECT checked_at FROM checkin_log WHERE iid = ?1
                         ORDER BY checked_at DESC LIMIT 1",
                        [iid],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(value)
            })
            .ok()
            .flatten();
        ms.and_then(ms_of)
    }
}

fn day_of(clock: &dyn Clock) -> String {
    clock.today().format("%Y-%m-%d").to_string()
}

impl CheckinStore for SqliteCheckin<'_> {
    /// 契约 1：接口说了算，接口没给才看本地。
    fn resolve_checked(&self, iid: &str, remote: Option<bool>, clock: &dyn Clock) -> bool {
        remote.unwrap_or_else(|| self.marked_today(iid, clock))
    }

    fn marked_today(&self, iid: &str, clock: &dyn Clock) -> bool {
        let day = day_of(clock);
        self.db
            .read(|conn| {
                let hit: Option<i64> = conn
                    .query_row(
                        "SELECT 1 FROM daily_marks WHERE iid = ?1 AND date = ?2",
                        rusqlite::params![iid, day],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(hit.is_some())
            })
            .unwrap_or(false)
    }

    /// 契约 2：**只写成功之后**才调用。失败写进来会让下一轮误判成已签。
    fn mark_today(&self, iid: &str, clock: &dyn Clock) -> Result<()> {
        let day = day_of(clock);
        let now = clock.now().timestamp_millis();
        self.db.write(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO daily_marks(iid, date) VALUES (?1, ?2)",
                rusqlite::params![iid, day],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO checkin_log(iid, checked_at) VALUES (?1, ?2)",
                rusqlite::params![iid, now],
            )?;
            Ok(())
        })
    }

    fn next_at(&self, iid: &str) -> Option<DateTime<Local>> {
        let raw = self.db.kv_get(&Self::next_at_key(iid)).ok().flatten()?;
        DateTime::parse_from_rfc3339(&raw)
            .ok()
            .map(|dt| dt.with_timezone(&Local))
    }

    fn note_next_at(&self, iid: &str, at: DateTime<Local>) -> Result<()> {
        self.db.kv_set(
            &Self::next_at_key(iid),
            &at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        )
    }

    /// 契约 4：`0` = 自然日（次日 0 点），`24` = 上次 + 24h。
    fn next_checkin_at(
        &self,
        cooldown_hours: u32,
        last: DateTime<Local>,
        _clock: &dyn Clock,
    ) -> Option<DateTime<Local>> {
        if cooldown_hours == 0 {
            let next_day = last.date_naive().succ_opt()?;
            return Local
                .from_local_datetime(&next_day.and_hms_opt(0, 0, 0)?)
                .single();
        }
        Some(last + chrono::Duration::hours(cooldown_hours as i64))
    }

    /// 契约 6：同日多次只留最后一次，且**用 upsert 不是 insert**。
    fn note_balance(&self, iid: &str, balance: f64, clock: &dyn Clock) -> Result<bool> {
        let day = day_of(clock);
        if self.balance_on(iid, &day) == Some(balance) {
            return Ok(false);
        }
        self.db.write(|conn| {
            conn.execute(
                "INSERT INTO credit_snapshots(iid, date, balance) VALUES (?1, ?2, ?3)
                 ON CONFLICT(iid, date) DO UPDATE SET balance = excluded.balance",
                rusqlite::params![iid, day, balance],
            )?;
            Ok(true)
        })
    }

    fn balance_on(&self, iid: &str, day: &str) -> Option<f64> {
        self.db
            .read(|conn| {
                let value = conn
                    .query_row(
                        "SELECT balance FROM credit_snapshots WHERE iid = ?1 AND date = ?2",
                        rusqlite::params![iid, day],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(value)
            })
            .ok()
            .flatten()
    }
}

/// 契约 6 的「今日增加」：今天与**上一次有记录的那天**的差值。
pub fn today_delta(db: &Db, iid: &str, today: NaiveDate) -> Option<f64> {
    let stamp = today.format("%Y-%m-%d").to_string();
    db.read(|conn| {
        let previous: Option<f64> = conn
            .query_row(
                "SELECT balance FROM credit_snapshots
                 WHERE iid = ?1 AND date < ?2 ORDER BY date DESC LIMIT 1",
                rusqlite::params![iid, stamp],
                |row| row.get(0),
            )
            .optional()?;
        let current: Option<f64> = conn
            .query_row(
                "SELECT balance FROM credit_snapshots WHERE iid = ?1 AND date = ?2",
                rusqlite::params![iid, stamp],
                |row| row.get(0),
            )
            .optional()?;
        Ok(match (previous, current) {
            (Some(previous), Some(current)) => Some(current - previous),
            _ => None,
        })
    })
    .ok()
    .flatten()
}

/// 最近一次实时查询看到的额度与批次。**只做展示缓存**，不参与台账计算：
/// 台账记的是「本工具亲眼发放」，这里记的是「平台此刻还剩什么」。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CreditView {
    /// 快照时刻（毫秒）。界面据此说「更新于 …」，而不是把旧值当实时值。
    pub at: i64,
    /// 额度型平台：窗口名 → 已用百分比
    pub quota: std::collections::BTreeMap<String, f64>,
    pub lots: Vec<crate::types::CreditLot>,
}

fn view_key(iid: &str) -> String {
    format!("credit_view:{iid}")
}

impl SqliteCheckin<'_> {
    /// 记下「平台说今天没签」。只存日期，跨天自动失效。
    pub fn note_unchecked(&self, iid: &str, clock: &dyn Clock) -> Result<()> {
        self.db.kv_set(&Self::unchecked_key(iid), &day_of(clock))
    }

    /// 平台今天明确说过「没签」——用来把列表的「待查询」变成「未签」。
    pub fn unchecked_today(&self, iid: &str, clock: &dyn Clock) -> bool {
        self.db
            .kv_get(&Self::unchecked_key(iid))
            .ok()
            .flatten()
            .map(|day| day == day_of(clock))
            .unwrap_or(false)
    }

    pub fn view_of(&self, iid: &str) -> Option<CreditView> {
        let raw = self.db.kv_get(&view_key(iid)).ok().flatten()?;
        serde_json::from_str(&raw).ok()
    }

    /// `None` = 本次没拿到这一项，保留上一次的值（接口偶尔只给余额不给批次）。
    pub fn note_view(
        &self,
        iid: &str,
        quota: Option<std::collections::BTreeMap<String, f64>>,
        lots: Option<Vec<crate::types::CreditLot>>,
        at: i64,
    ) -> Result<()> {
        let mut view = self.view_of(iid).unwrap_or_default();
        if let Some(quota) = quota {
            view.quota = quota;
        }
        if let Some(lots) = lots {
            view.lots = lots;
        }
        view.at = at;
        let text = serde_json::to_string(&view).map_err(|e| {
            crate::app::error::AppError::new(ErrorKind::Invalid, format!("配额缓存序列化失败：{e}"))
        })?;
        self.db.kv_set(&view_key(iid), &text)
    }
}

/// WorkBuddy 国际版没有可靠的批次到期（CycleEndTime 是周期重置，不是到期）。
/// 旧缓存里可能还留着按它算出来的日期，展示前一律丢掉：宁可空着，也不显示错的。
pub fn lots_for_display(
    iid: &str,
    lots: Vec<crate::types::CreditLot>,
) -> Vec<crate::types::CreditLot> {
    if is_intl_account(iid) {
        lots.into_iter()
            .map(|mut lot| {
                lot.expires_at = None;
                lot
            })
            .collect()
    } else {
        lots
    }
}

/// 最近一个还没到期的批次，以及**同一天**到期的合计金额。
fn nearest_lot(
    lots: &[crate::types::CreditLot],
    now: DateTime<Local>,
) -> (Option<i64>, Option<f64>) {
    let nearest = lots
        .iter()
        .filter_map(|lot| lot.expires_at)
        .filter(|exp| *exp > now)
        .min();
    let Some(day) = nearest else {
        return (None, None);
    };
    let amount = lots
        .iter()
        .filter(|lot| lot.expires_at.map(|exp| exp.date_naive()) == Some(day.date_naive()))
        .map(|lot| lot.amount)
        .sum();
    (Some(day.timestamp_millis()), Some(amount))
}

/// 把库里已有的状态补进账号视图。
///
/// 契约 21 的落地方式：余额与下次可签时刻**从库里读**，不是从某次查询的返回值里
/// 攒出来的。于是重扫、刷新、重启都不会把它们冲成「—」。
pub fn hydrate(db: &Db, rows: &mut [crate::app::accounts::AccountView], clock: &dyn Clock) {
    let store = SqliteCheckin::new(db);
    let today = clock.today().format("%Y-%m-%d").to_string();
    let now = clock.now();
    for row in rows {
        // 三态：本工具签过 / 平台今天说没签 / 没问过。前两个才配显示结论，
        // 最后一个老实说「待查询」——别把「不知道」画成「未签」。
        row.checked_in = if store.marked_today(&row.iid, clock) {
            Some(true)
        } else if store.unchecked_today(&row.iid, clock) {
            Some(false)
        } else {
            None
        };
        row.balance = store.balance_on(&row.iid, &today);
        row.next_at = store.next_at(&row.iid).map(|at| at.timestamp_millis());
        if row.checked_in == Some(true) {
            // 显示「HH:MM · 成功」用真实签到时刻；拿刷新时刻顶替会随每次打开漂移
            row.checked_at = store
                .last_checkin_at(&row.iid)
                .map(|at| at.timestamp_millis());
        }
        if let Some(view) = store.view_of(&row.iid) {
            // 空配额 = 这个平台没有额度概念，不是「额度用完了」：别让配额列显示「无额度」
            if !view.quota.is_empty() {
                row.quota = view.quota;
                row.quota_at = Some(view.at);
            }
            (row.lot_expires_at, row.lot_amount) =
                nearest_lot(&lots_for_display(&row.iid, view.lots), now);
        }
        if row.lot_expires_at.is_none() && !is_intl_account(&row.iid) {
            // 没有实时批次（免签到平台 / 还没刷过）：退回本工具发放台账。
            // 国际版不参与：它的批次到期本身不可信，台账里的旧日期也不该翻出来。
            (row.lot_expires_at, row.lot_amount) = nearest_lot(&lots_of(db, &row.iid, 30), now);
        }
    }
}

/// 本地记下的发放批次，最近的排前面。详情面板只展示，不参与计算。
pub fn lots_of(db: &Db, iid: &str, limit: usize) -> Vec<crate::types::CreditLot> {
    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT amount, expires_at, granted_at, lot_key FROM credit_lots
             WHERE iid = ?1 ORDER BY granted_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![iid, limit as i64], |row| {
            let expires: Option<i64> = row.get(1)?;
            let granted: Option<i64> = row.get(2)?;
            Ok(crate::types::CreditLot {
                amount: row.get(0)?,
                expires_at: expires.and_then(ms_of),
                granted_at: granted.and_then(ms_of),
                lot_key: row.get(3)?,
                label: String::new(),
            })
        })?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item?);
        }
        Ok(out)
    })
    .unwrap_or_default()
}

fn ms_of(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

pub struct SqliteLedger<'a> {
    db: &'a Db,
}

impl<'a> SqliteLedger<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    /// 记一个带到期日的发放批次（`validity_days` 为 None 时不设到期日）。
    pub fn record(
        &self,
        iid: &str,
        amount: f64,
        expires_at: Option<DateTime<Local>>,
        validity_days: Option<i64>,
        clock: &dyn Clock,
    ) -> Result<bool> {
        let expires =
            expires_at.or_else(|| validity_days.map(|d| clock.now() + chrono::Duration::days(d)));
        let key = format!("{iid}:{}:{amount}", clock.now().timestamp_millis());
        self.db.write(|conn| {
            let rows = conn.execute(
                "INSERT OR IGNORE INTO credit_lots(iid, amount, expires_at, granted_at, lot_key)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    iid,
                    amount,
                    expires.map(|e| e.timestamp_millis()),
                    clock.now().timestamp_millis(),
                    key
                ],
            )?;
            Ok(rows > 0)
        })
    }
}

impl Ledger for SqliteLedger<'_> {
    /// 契约 5：`live_credit` 为真表示平台自己会返回批次，写台账只会重复记一份。
    fn add_if_needed(
        &self,
        live_credit: bool,
        iid: &str,
        granted: f64,
        validity_days: Option<i64>,
        clock: &dyn Clock,
    ) -> Result<bool> {
        if live_credit {
            return Ok(false);
        }
        self.record(iid, granted, None, validity_days, clock)
    }

    fn count(&self, iid: &str) -> usize {
        self.db
            .read(|conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM credit_lots WHERE iid = ?1",
                    [iid],
                    |row| row.get(0),
                )?;
                Ok(count as usize)
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::accounts::{repo, AccountIdentity};
    use crate::app::clock::FakeClock;
    use crate::types::CreditLot;

    fn identity(iid: &str, platform: &str) -> AccountIdentity {
        AccountIdentity {
            iid: iid.into(),
            platform: platform.into(),
            platform_label: platform.into(),
            account_id: iid.split_once(':').unwrap().1.into(),
            name: "测试".into(),
            requires_checkin: true,
            has_secret: true,
            ..Default::default()
        }
    }

    /// 配额与批次到期必须能从缓存回到账号视图：否则表格永远是「—」。
    #[test]
    fn hydrate_restores_quota_and_nearest_lot() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        repo::upsert_identity(&db, &identity("zcode:u1", "zcode"), 1).unwrap();
        let clock = FakeClock::at("2026-11-02T09:00:00+08:00");
        let lot = |amount: f64, days: i64, key: &str| CreditLot {
            amount,
            expires_at: Some(clock.now() + chrono::Duration::days(days)),
            granted_at: None,
            lot_key: key.into(),
            label: "资源包".into(),
        };
        SqliteCheckin::new(&db)
            .note_view(
                "zcode:u1",
                Some(std::collections::BTreeMap::from([(
                    "周额度".to_string(),
                    42.0,
                )])),
                Some(vec![
                    lot(30.0, 9, "b"),
                    lot(120.0, 3, "a"),
                    lot(50.0, 3, "c"),
                ]),
                1_760_000_000_000,
            )
            .unwrap();

        let mut rows = repo::list(&db).unwrap();
        hydrate(&db, &mut rows, &clock);
        let row = &rows[0];
        assert_eq!(row.quota.get("周额度"), Some(&42.0));
        assert_eq!(row.quota_at, Some(1_760_000_000_000));
        // 最近的是 3 天后那两笔，金额按「同一天到期」合计
        let expected = (clock.now() + chrono::Duration::days(3)).timestamp_millis();
        assert_eq!(row.lot_expires_at, Some(expected));
        assert_eq!(row.lot_amount, Some(170.0));
    }

    /// 没有配额窗口的平台不该被写成「无额度」。
    #[test]
    fn hydrate_keeps_quota_empty_for_non_quota_platforms() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        repo::upsert_identity(&db, &identity("qoder:u1", "qoder"), 1).unwrap();
        let clock = FakeClock::at("2026-11-02T09:00:00+08:00");
        SqliteCheckin::new(&db)
            .note_view("qoder:u1", None, Some(Vec::new()), 1_760_000_000_000)
            .unwrap();

        let mut rows = repo::list(&db).unwrap();
        hydrate(&db, &mut rows, &clock);
        assert!(rows[0].quota.is_empty());
        assert_eq!(rows[0].quota_at, None);
    }

    /// 国际版的批次到期不可信：旧缓存里的日期也不能再显示。
    #[test]
    fn intl_lots_are_shown_without_expiry() {
        let clock = FakeClock::at("2026-11-02T09:00:00+08:00");
        let lot = CreditLot {
            amount: 10.0,
            expires_at: Some(clock.now() + chrono::Duration::days(3)),
            granted_at: None,
            lot_key: "x".into(),
            label: "资源包".into(),
        };
        let shown = lots_for_display("workbuddy:u1@intl", vec![lot.clone()]);
        assert!(shown[0].expires_at.is_none(), "国际版不显示批次到期");
        let kept = lots_for_display("workbuddy:u1", vec![lot]);
        assert!(kept[0].expires_at.is_some(), "国内版照常显示");
    }
}
