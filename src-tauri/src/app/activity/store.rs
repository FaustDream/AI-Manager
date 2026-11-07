//! 账号级旅行偏好与本机旅行记录的存储。放 kv 而不是 `accounts` 表：这是活动配置，
//! 重扫只更新身份字段，不应该被平台昵称/凭据刷新一起冲掉。

use super::{TravelEvent, TravelPrefs};
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use std::collections::BTreeMap;

const PREFS_KV: &str = "activity.travel.prefs";
/// 本机旅行记录：派出/回来/领取。平台接口查不到这些时刻，只能自己记。
const EVENTS_KV: &str = "activity.travel.events";
/// 每个账号最多保留多少条：够回看最近几周，也不会把 kv 撑大。
const EVENTS_LIMIT: usize = 50;

fn map_of(db: &Db) -> BTreeMap<String, TravelPrefs> {
    db.kv_get(PREFS_KV)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn prefs_of(db: &Db, iid: &str) -> TravelPrefs {
    map_of(db).remove(iid).unwrap_or_default()
}

pub fn all(db: &Db) -> BTreeMap<String, TravelPrefs> {
    map_of(db)
}

pub fn set(db: &Db, iid: &str, prefs: TravelPrefs) -> Result<TravelPrefs> {
    let mut map = map_of(db);
    map.insert(iid.to_string(), prefs.clone());
    let text = serde_json::to_string(&map)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("保存旅行配置失败：{e}")))?;
    db.kv_set(PREFS_KV, &text)?;
    Ok(prefs)
}

fn event_map(db: &Db) -> BTreeMap<String, Vec<TravelEvent>> {
    db.kv_get(EVENTS_KV)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 本机旅行记录，最新的在前。
pub fn events_of(db: &Db, iid: &str) -> Vec<TravelEvent> {
    let mut events = event_map(db).remove(iid).unwrap_or_default();
    events.sort_by_key(|item| std::cmp::Reverse(item.at));
    events
}

/// 记一条旅行事件。同一 (kind, at) 只记一条——「回来」由多处以同一平台的
/// `arrive_at` 反复观察到，按时刻去重才不会每次刷新都多出一行。
pub fn record_event(db: &Db, iid: &str, event: TravelEvent) {
    let mut map = event_map(db);
    let events = map.entry(iid.to_string()).or_default();
    let duplicated = events
        .iter()
        .any(|item| item.kind == event.kind && item.at == event.at);
    if duplicated {
        return;
    }
    events.push(event);
    events.sort_by_key(|item| std::cmp::Reverse(item.at));
    events.truncate(EVENTS_LIMIT);
    if let Ok(text) = serde_json::to_string(&map) {
        let _ = db.kv_set(EVENTS_KV, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_roundtrip_and_default_off() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        assert!(!prefs_of(&db, "workbuddy:u1").enabled, "默认不参与自动旅行");

        set(
            &db,
            "workbuddy:u1",
            TravelPrefs {
                enabled: true,
                location_id: Some(1),
            },
        )
        .unwrap();
        let prefs = prefs_of(&db, "workbuddy:u1");
        assert!(prefs.enabled);
        assert_eq!(prefs.location_id, Some(1));
        assert_eq!(all(&db).len(), 1);
    }

    #[test]
    fn events_dedup_by_kind_and_time_and_cap() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let event = |at: i64| TravelEvent {
            at,
            kind: "arrive".into(),
            location: "溪谷草地".into(),
            amount: None,
        };
        record_event(&db, "workbuddy:u1", event(1000));
        record_event(&db, "workbuddy:u1", event(1000));
        record_event(&db, "workbuddy:u1", event(2000));
        assert_eq!(
            events_of(&db, "workbuddy:u1").len(),
            2,
            "同 kind 同时刻去重"
        );
        assert_eq!(events_of(&db, "workbuddy:u1")[0].at, 2000, "最新的在前");

        for index in 0..(EVENTS_LIMIT + 10) {
            record_event(
                &db,
                "workbuddy:u1",
                TravelEvent {
                    at: 10_000 + index as i64,
                    kind: "depart".into(),
                    location: "云上栈道".into(),
                    amount: None,
                },
            );
        }
        let events = events_of(&db, "workbuddy:u1");
        assert_eq!(events.len(), EVENTS_LIMIT, "超出上限时丢最旧的");
        assert_eq!(events[0].at, 10_000 + (EVENTS_LIMIT + 10) as i64 - 1);
    }
}
