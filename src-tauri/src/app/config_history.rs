//! 配置快照与回滚。
//!
//! 定时任务、签到节奏这类配置写坏了没有「撤销」：改错一次只能手改库。
//! 这里在每次写配置**之前**把旧值留一份，设置页可以看历史并回滚。
//!
//! 存 `kv` 的 `config.snapshots`（JSON 数组，保留最近 30 条）。
//! `ui.*` 是界面状态（列宽、主题），改一次存一条只会把历史刷满，不参与快照。

use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use serde::{Deserialize, Serialize};

const KV: &str = "config.snapshots";
const KEEP: usize = 30;
/// 单条快照的文本上限：配置文件不该有这么大，超过就截断而不是把库撑爆
const MAX_VALUE: usize = 256 * 1024;

/// 一次配置变更。`before` 是回滚要写回去的内容，`after` 只用于界面展示「改成了什么」。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    /// 被改的 kv 键（`schedules` / `checkin.gap_ms` …）
    pub key: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at: i64,
    /// 变更前的内容；`None` 表示这个键当时不存在
    pub before: Option<String>,
    pub after: Option<String>,
    /// 变更前内容的字节数，列表里用来判断「这条值不值得看」
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub before_bytes: usize,
}

/// 这个键要不要留快照。界面状态不参与。
fn tracked(key: &str) -> bool {
    !key.starts_with("ui.") && key != KV
}

fn load(db: &Db) -> Result<Vec<Snapshot>> {
    match db.kv_get(KV)? {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .map_err(|e| AppError::new(ErrorKind::Invalid, format!("配置快照解析失败：{e}"))),
        _ => Ok(Vec::new()),
    }
}

fn save(db: &Db, list: &[Snapshot]) -> Result<()> {
    let text = serde_json::to_string(list)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("配置快照序列化失败：{e}")))?;
    db.kv_set(KV, &text)
}

fn clip(text: &str) -> String {
    if text.len() <= MAX_VALUE {
        return text.to_string();
    }
    let mut cut = MAX_VALUE;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…（已截断）", &text[..cut])
}

/// 写配置前调用：把「当前值」留一份，再让调用方去写新值。
///
/// 值没变就不留（避免重复保存刷出一堆一样的快照）。
pub fn capture(db: &Db, key: &str, after: Option<&str>) -> Result<()> {
    if !tracked(key) {
        return Ok(());
    }
    let before = db.kv_get(key)?;
    if before.as_deref() == after {
        return Ok(());
    }
    // 超过上限的配置不留快照：截断后写回去等于把配置写坏，宁可没有这条历史
    if let Some(text) = &before {
        if text.len() > MAX_VALUE {
            crate::app::log::log(
                crate::app::log::Level::Warn,
                "config",
                format!(
                    "配置 {key} 超过快照上限（{} 字节），本次未留快照",
                    text.len()
                ),
            );
            return Ok(());
        }
    }
    let at = crate::app::timeutil::now_ms();
    let mut list = load(db)?;
    list.push(Snapshot {
        id: format!("{at}-{}", list.len()),
        key: key.to_string(),
        at,
        before_bytes: before.as_deref().map(str::len).unwrap_or(0),
        before,
        after: after.map(clip),
    });
    // 只留最近 KEEP 条：快照是回退依据，不是审计档案
    if list.len() > KEEP {
        let drop = list.len() - KEEP;
        list.drain(0..drop);
    }
    save(db, &list)
}

pub fn list(db: &Db) -> Result<Vec<Snapshot>> {
    let mut list = load(db)?;
    // 新的在前：用户要找的几乎总是刚才那一次
    list.reverse();
    Ok(list)
}

/// 回滚到某条快照之前的状态。回滚本身也留一条快照，滚错了还能滚回来。
pub fn restore(db: &Db, id: &str) -> Result<Snapshot> {
    let list = load(db)?;
    let target = list
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "这条快照不存在，可能已被清理"))?;
    // 先按「改回 before」记一条，回滚动作本身才可撤销
    capture(db, &target.key, target.before.as_deref())?;
    match &target.before {
        Some(text) => db.kv_set(&target.key, text)?,
        None => db.kv_delete(&target.key)?,
    }
    Ok(target)
}

pub fn clear(db: &Db) -> Result<()> {
    db.kv_delete(KV)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn capture_keeps_previous_value_and_skips_noop() {
        let (_dir, db) = db();
        db.kv_set("schedules", "old").unwrap();
        capture(&db, "schedules", Some("new")).unwrap();
        let rows = list(&db).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].before.as_deref(), Some("old"));
        assert_eq!(rows[0].after.as_deref(), Some("new"));

        // 值没变不该再留一条
        capture(&db, "schedules", Some("old")).unwrap();
        assert_eq!(list(&db).unwrap().len(), 1);
    }

    #[test]
    fn ui_keys_are_not_tracked() {
        let (_dir, db) = db();
        db.kv_set("ui.columns", "{}").unwrap();
        capture(&db, "ui.columns", Some("{\"a\":1}")).unwrap();
        assert!(list(&db).unwrap().is_empty(), "界面状态不占配置历史");
    }

    #[test]
    fn restore_writes_before_and_keeps_undo() {
        let (_dir, db) = db();
        db.kv_set("checkin.gap_ms", "100").unwrap();
        capture(&db, "checkin.gap_ms", Some("300")).unwrap();
        db.kv_set("checkin.gap_ms", "300").unwrap();

        let id = list(&db).unwrap()[0].id.clone();
        let restored = restore(&db, &id).unwrap();
        assert_eq!(restored.key, "checkin.gap_ms");
        assert_eq!(db.kv_get("checkin.gap_ms").unwrap().as_deref(), Some("100"));
        // 回滚本身也留痕：再回滚一次能回到 300
        let rows = list(&db).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].before.as_deref(), Some("300"));
    }

    #[test]
    fn restore_missing_key_deletes_it() {
        let (_dir, db) = db();
        capture(&db, "brand.new", Some("x")).unwrap();
        db.kv_set("brand.new", "x").unwrap();
        let id = list(&db).unwrap()[0].id.clone();
        restore(&db, &id).unwrap();
        assert!(
            db.kv_get("brand.new").unwrap().is_none(),
            "原来不存在就应删掉"
        );
    }

    #[test]
    fn history_is_capped() {
        let (_dir, db) = db();
        for index in 0..(KEEP + 5) {
            capture(&db, "k", Some(&format!("v{index}"))).unwrap();
        }
        assert_eq!(list(&db).unwrap().len(), KEEP);
    }
}
