//! 账号级旅行偏好的存储。放 kv 而不是 `accounts` 表：这是活动配置，
//! 重扫只更新身份字段，不应该被平台昵称/凭据刷新一起冲掉。

use super::TravelPrefs;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use std::collections::BTreeMap;

const PREFS_KV: &str = "activity.travel.prefs";

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
}
