//! 会话维度的统计（仪表盘的「会话」区块）。
//!
//! 口径说明：这里的 total / by_source / by_project 是**全部未隐藏**的会话
//! （含已归档——归档也是资产）；today / week / unnamed 按活跃口径
//! （不含已归档），与「去会话页」按钮落地的列表视图对得上。

use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::sessions::store::{project_name, UNNAMED_TITLE};
use chrono::{Local, TimeZone};

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SessionStats {
    /// 今天有活动的会话数（活跃口径）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub today: usize,
    /// 最近 7 天有活动的会话数（活跃口径）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub week: usize,
    /// 会话总数（未隐藏，含已归档）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
    /// 未命名会话数（活跃口径，「去处理」落到未命名快筛）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub unnamed: usize,
    /// 已归档会话数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub archived: usize,
    /// 按来源计数（含已归档）
    pub by_source: Vec<SourceCount>,
    /// 按项目计数 Top N（含已归档）
    pub by_project: Vec<ProjectCount>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceCount {
    pub source_key: String,
    pub label: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: usize,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectCount {
    pub project: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: usize,
}

/// 项目排名的条数上限：仪表盘的排名列表超过 8 行就该滚动而不是铺开。
const TOP_PROJECTS: usize = 8;

pub fn stats(db: &Db) -> Result<SessionStats> {
    let now_ms = Local::now().timestamp_millis();
    let day_start = start_of_today_ms();
    let week_start = now_ms - 7 * 24 * 3600 * 1000;

    let mut out = SessionStats::default();
    db.read(|conn| {
        // 活跃口径的行集合：未隐藏且未归档
        let active_sql = "FROM session_index s
             LEFT JOIN session_marks m
               ON m.source_key = s.source_key AND m.session_id = s.session_id
             WHERE m.hidden_at IS NULL";
        out.today = conn.query_row(
            &format!("SELECT COUNT(*) {active_sql} AND COALESCE(s.last_at, 0) >= ?1 AND COALESCE(m.archived_at, '') = ''"),
            [day_start],
            |r| r.get::<_, i64>(0),
        )? as usize;
        out.week = conn.query_row(
            &format!("SELECT COUNT(*) {active_sql} AND COALESCE(s.last_at, 0) >= ?1 AND COALESCE(m.archived_at, '') = ''"),
            [week_start],
            |r| r.get::<_, i64>(0),
        )? as usize;
        out.total = conn.query_row(
            &format!("SELECT COUNT(*) {active_sql}"),
            [],
            |r| r.get::<_, i64>(0),
        )? as usize;
        out.unnamed = conn.query_row(
            &format!(
                "SELECT COUNT(*) {active_sql}
                 AND COALESCE(m.archived_at, '') = ''
                 AND (s.title = ?1 OR TRIM(s.title) = '')"
            ),
            [UNNAMED_TITLE],
            |r| r.get::<_, i64>(0),
        )? as usize;
        out.archived = conn.query_row(
            &format!(
                "SELECT COUNT(*) {active_sql} AND m.archived_at IS NOT NULL"
            ),
            [],
            |r| r.get::<_, i64>(0),
        )? as usize;

        // 按来源
        let mut stmt = conn.prepare(&format!(
            "SELECT s.source_key, s.source_label, COUNT(*) {active_sql}
             GROUP BY s.source_key, s.source_label ORDER BY COUNT(*) DESC, s.source_label"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(SourceCount {
                source_key: row.get(0)?,
                label: row.get(1)?,
                count: row.get::<_, i64>(2)? as usize,
            })
        })?;
        for row in rows {
            out.by_source.push(row?);
        }

        // 按项目：项目名要过 project_name 的归一化（slug 优先、哈希忽略），
        // 交给 Rust 聚合而不是在 SQL 里复刻这套规则
        let mut stmt = conn.prepare(&format!(
            "SELECT s.slug, s.cwd {active_sql}"
        ))?;
        let rows = stmt.query_map([], |row| {
            let slug: String = row.get(0)?;
            let cwd: String = row.get(1)?;
            Ok(project_name(&slug, &cwd))
        })?;
        let mut projects: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in rows {
            let project = row?;
            if project.is_empty() {
                continue;
            }
            *projects.entry(project).or_default() += 1;
        }
        let mut list: Vec<ProjectCount> = projects
            .into_iter()
            .map(|(project, count)| ProjectCount { project, count })
            .collect();
        list.sort_by(|a, b| b.count.cmp(&a.count).then(a.project.cmp(&b.project)));
        list.truncate(TOP_PROJECTS);
        out.by_project = list;

        Ok(out)
    })
}

/// 本地时区「今天 00:00」的毫秒时间戳。
fn start_of_today_ms() -> i64 {
    let midnight = Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_else(|| Local::now().date_naive().and_hms_opt(12, 0, 0).unwrap());
    Local
        .from_local_datetime(&midnight)
        .single()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::sessions::store::RawRow;

    /// 一次性把整源种进索引：replace_source 是**整源替换**，分次调用会互相清空。
    fn seed(db: &Db, rows: &[(&str, &str, i64, &str)]) {
        let mapped: Vec<RawRow> = rows
            .iter()
            .map(|(sid, title, last_at, state)| {
                let mut row = RawRow::empty("qoder", "qoder_cn", "Qoder CN");
                row.session_id = (*sid).into();
                row.title = (*title).into();
                row.last_at = *last_at;
                row.state = match *state {
                    "deleted" => crate::types::SessionState::Deleted,
                    "orphan" => crate::types::SessionState::Orphan,
                    _ => crate::types::SessionState::Visible,
                };
                row
            })
            .collect();
        crate::app::sessions::store::SqliteSessions::new(db)
            .replace_source("qoder_cn", &mapped)
            .unwrap();
    }

    #[test]
    fn counts_today_week_and_unnamed() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let now = Local::now().timestamp_millis();
        seed(
            &db,
            &[
                ("today", "今天的", now, "visible"),
                ("old", "很久前的", now - 30 * 24 * 3600 * 1000, "visible"),
                ("unnamed", UNNAMED_TITLE, now, "visible"),
                ("gone", "已隐藏", now, "visible"),
            ],
        );
        crate::app::sessions::marks::hide(&db, &["qoder|qoder_cn||gone".to_string()]).unwrap();

        let s = stats(&db).unwrap();
        assert_eq!(s.today, 2, "今天活动：今天的 + 未命名");
        assert_eq!(s.week, 2);
        assert_eq!(s.total, 3, "隐藏行不算总数");
        assert_eq!(s.unnamed, 1);
        assert_eq!(s.by_source.len(), 1);
        assert_eq!(s.by_source[0].count, 3);
    }
}
