//! 会话维度的统计（仪表盘的「会话」区块）。
//!
//! 口径说明：
//!   - `active` / `by_source` / `by_project` 是**区间口径**——窗口内有过活动
//!     （`last_at` 落在区间里）且未归档的会话，三者分母一致，占比才对得上；
//!   - `total` / `archived` / `unnamed` 是**存量口径**（未隐藏，含已归档）：
//!     库存数字跟区间无关，换区间时不该变。
//!
//! 窗口由界面给定（`app/window.rs`），不设天数上限。

use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::sessions::store::{project_name, UNNAMED_TITLE};
use crate::app::window::DayWindow;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SessionStats {
    /// 区间内有活动的会话数（活跃口径）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub active: usize,
    /// 会话总数（未隐藏，含已归档）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: usize,
    /// 未命名会话数（存量口径，未归档）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub unnamed: usize,
    /// 已归档会话数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub archived: usize,
    /// 按来源计数（区间口径）
    pub by_source: Vec<SourceCount>,
    /// 按项目计数 Top N（区间口径）
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

pub fn stats(db: &Db, window: DayWindow) -> Result<SessionStats> {
    let (start, end) = (window.start_ms(), window.end_ms());

    let mut out = SessionStats::default();
    db.read(|conn| {
        // 活跃口径的行集合：未隐藏且未归档
        let active_sql = "FROM session_index s
             LEFT JOIN session_marks m
               ON m.source_key = s.source_key AND m.session_id = s.session_id
             WHERE m.hidden_at IS NULL";
        // 区间条件：`last_at` 落在 [start, end]。`?1`/`?2` 在每条语句里都对应 (start, end)
        let window_sql =
            " AND COALESCE(s.last_at, 0) >= ?1 AND COALESCE(s.last_at, 0) <= ?2 AND COALESCE(m.archived_at, '') = ''";
        out.active = conn.query_row(
            &format!("SELECT COUNT(*) {active_sql}{window_sql}"),
            [start, end],
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
            "SELECT s.source_key, s.source_label, COUNT(*) {active_sql}{window_sql}
             GROUP BY s.source_key, s.source_label ORDER BY COUNT(*) DESC, s.source_label"
        ))?;
        let rows = stmt.query_map([start, end], |row| {
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
            "SELECT s.slug, s.cwd {active_sql}{window_sql}"
        ))?;
        let rows = stmt.query_map([start, end], |row| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::sessions::store::RawRow;
    use chrono::Local;

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
    fn counts_active_in_window_and_inventory_totals() {
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

        // 近 7 天：今天的 + 未命名，30 天前那条不算
        let week = stats(&db, DayWindow::last_days(7)).unwrap();
        assert_eq!(week.active, 2, "区间活跃：今天的 + 未命名");
        assert_eq!(week.total, 3, "隐藏行不算总数");
        assert_eq!(week.unnamed, 1);
        assert_eq!(week.by_source.len(), 1);
        assert_eq!(week.by_source[0].count, 2, "来源排名与区间活跃同分母");

        // 拉到 60 天：30 天前那条也进来
        let wide = stats(&db, DayWindow::last_days(60)).unwrap();
        assert_eq!(wide.active, 3);
        assert_eq!(wide.by_source[0].count, 3);
    }
}
