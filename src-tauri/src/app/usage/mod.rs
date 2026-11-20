//! 本地用量采集：**只读**客户端落盘文件，不代理、不注入、不改任何配置。
//!
//! 数据来源：
//!   - WorkBuddy 国内版/国际版：`projects/<slug>/<id>.jsonl`，每条 assistant 消息带
//!     `providerData.usage` 与 `rawUsage`（含平台实际扣费 `credit`）
//!   - CodeBuddy：`history/<ws>/index.json` 的 `requests[].usage`（同样带 `credit`）
//!   - ZCode：`cli/db/db.sqlite` 的 `model_usage` 表（只有 token，没有扣费）
//!   - Qoder：只有上下文占用快照（`chat_session_context_usage`），没有逐次计费
//!   - Trae：本地 `database.db` 是加密格式，读不到
//!
//! 增量策略：JSONL 记文件偏移、index.json 记 mtime+size、SQLite 记 `started_at` 游标。
//! 全部写入按 `record_id` 主键 `INSERT OR IGNORE`，重扫不会重复计数。

use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::Result;
use crate::app::window::DayWindow;
use serde::Serialize;
use std::collections::BTreeMap;

mod codebuddy;
mod doubao;
mod qoder;
mod trae;
mod workbuddy;
mod zcode;

const LAST_SCAN_KV: &str = "usage.last_scan";

/// 一条用量记录。`record_id` 是源内唯一键（主键即幂等）。
struct Record {
    record_id: String,
    platform: String,
    model: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
    credit: Option<f64>,
    created_at: i64,
    source: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Default, Clone, Serialize)]
pub struct UsageScanReport {
    /// 解析到的记录数（含重复）
    pub scanned: usize,
    /// 实际新写入的条数
    pub inserted: usize,
    /// 跳过的源（读不到/格式不符），不视为失败
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
    pub ms: u32,
}

/// 平台覆盖情况：界面直接展示，避免把「没数据」误解成「没统计」。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct SourceCoverage {
    pub platform: String,
    pub label: String,
    /// `full` 有 token+扣费 / `tokens` 只有 token / `context` 只有上下文占用 / `none` 不可读
    pub level: String,
    pub note: String,
}

pub fn coverage() -> Vec<SourceCoverage> {
    let mut out = Vec::new();
    out.extend(workbuddy::coverage());
    out.extend(codebuddy::coverage());
    out.extend(zcode::coverage());
    out.extend(qoder::coverage());
    out.extend(trae::coverage());
    out.extend(doubao::coverage());
    out
}

// ---------------------------------------------------------------- 采集入口

pub fn scan_all(db: &Db, roots: &Roots) -> Result<UsageScanReport> {
    let started = std::time::Instant::now();
    let mut report = UsageScanReport::default();
    workbuddy::scan_workbuddy(db, roots, &mut report);
    codebuddy::scan_codebuddy(db, roots, &mut report);
    zcode::scan_zcode(db, roots, &mut report);
    report.ms = started.elapsed().as_millis() as u32;
    let _ = db.kv_set(LAST_SCAN_KV, &crate::app::timeutil::now_ms().to_string());
    Ok(report)
}

fn insert_many(db: &Db, records: &[Record], report: &mut UsageScanReport) {
    if records.is_empty() {
        return;
    }
    report.scanned += records.len();
    let result = db.write(|conn| {
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO token_usage_records
             (record_id, platform, model, prompt_tokens, completion_tokens,
              total_tokens, credit, created_at, source)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        )?;
        let mut inserted = 0usize;
        for record in records {
            inserted += stmt.execute(rusqlite::params![
                record.record_id,
                record.platform,
                record.model,
                record.prompt_tokens,
                record.completion_tokens,
                record.total_tokens,
                record.credit,
                record.created_at,
                record.source,
            ])?;
        }
        Ok(inserted)
    });
    match result {
        Ok(count) => report.inserted += count,
        Err(e) => report.errors.push(format!("写入用量失败：{}", e.message)),
    }
}

// ---------------------------------------------------------------- 聚合

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct UsageSlice {
    pub platform: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub tokens: i64,
    pub credit: f64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub calls: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct UsageDay {
    pub date: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub tokens: i64,
    pub credit: f64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub calls: i64,
    pub by_app: Vec<UsageSlice>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct UsageModel {
    pub model: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub tokens: i64,
    pub credit: f64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub calls: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct UsageTotals {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub tokens: i64,
    pub credit: f64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub calls: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct UsageOverview {
    pub days: Vec<UsageDay>,
    pub by_app: Vec<UsageSlice>,
    pub by_model: Vec<UsageModel>,
    pub today: UsageTotals,
    pub total: UsageTotals,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub scanned_at: Option<i64>,
    pub coverage: Vec<SourceCoverage>,
}

/// 区间总览：按天 / 按平台 / 按模型。窗口由界面给定（`window.rs`），不设天数上限。
pub fn overview(db: &Db, window: DayWindow) -> Result<UsageOverview> {
    let today = chrono::Local::now().date_naive();
    let (since, until) = (window.start_ms(), window.end_ms());

    let mut day_map: BTreeMap<String, UsageDay> = BTreeMap::new();
    // 先把区间内每一天都铺成 0 值，图表才有完整的日期轴。
    // 只返回「有记录的天」会让相邻两点之间凭空连成斜线，看着像每天都在匀速上涨。
    let mut cursor = window.from;
    loop {
        let key = cursor.format("%Y-%m-%d").to_string();
        day_map.insert(
            key.clone(),
            UsageDay {
                date: key,
                tokens: 0,
                credit: 0.0,
                calls: 0,
                by_app: Vec::new(),
            },
        );
        if cursor >= window.to {
            break;
        }
        match cursor.succ_opt() {
            Some(next) => cursor = next,
            None => break,
        }
    }

    let mut app_map: BTreeMap<String, UsageSlice> = BTreeMap::new();
    let mut model_list: Vec<UsageModel> = Vec::new();
    let mut total = UsageTotals {
        tokens: 0,
        credit: 0.0,
        calls: 0,
    };
    let mut today_totals = UsageTotals {
        tokens: 0,
        credit: 0.0,
        calls: 0,
    };

    db.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT date(created_at/1000,'unixepoch','localtime') AS d, platform,
                    SUM(total_tokens), COALESCE(SUM(credit),0), COUNT(*)
             FROM token_usage_records WHERE created_at >= ?1 AND created_at <= ?2
             GROUP BY d, platform ORDER BY d",
        )?;
        let rows = stmt.query_map([since, until], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows.flatten() {
            let (date, platform, tokens, credit, calls) = row;
            let slice = UsageSlice {
                platform: platform.clone(),
                tokens,
                credit,
                calls,
            };
            let day = day_map.entry(date.clone()).or_insert_with(|| UsageDay {
                date,
                tokens: 0,
                credit: 0.0,
                calls: 0,
                by_app: Vec::new(),
            });
            day.tokens += slice.tokens;
            day.credit += slice.credit;
            day.calls += slice.calls;
            day.by_app.push(slice.clone());

            let app = app_map.entry(platform).or_insert_with(|| UsageSlice {
                platform: slice.platform.clone(),
                tokens: 0,
                credit: 0.0,
                calls: 0,
            });
            app.tokens += slice.tokens;
            app.credit += slice.credit;
            app.calls += slice.calls;

            total.tokens += slice.tokens;
            total.credit += slice.credit;
            total.calls += slice.calls;
            if day.date == today.format("%Y-%m-%d").to_string() {
                today_totals.tokens += slice.tokens;
                today_totals.credit += slice.credit;
                today_totals.calls += slice.calls;
            }
        }

        let mut stmt = conn.prepare(
            "SELECT model, SUM(total_tokens), COALESCE(SUM(credit),0), COUNT(*)
             FROM token_usage_records WHERE created_at >= ?1 AND created_at <= ?2
             GROUP BY model ORDER BY SUM(total_tokens) DESC LIMIT 20",
        )?;
        let rows = stmt.query_map([since, until], |row| {
            Ok(UsageModel {
                model: row.get(0)?,
                tokens: row.get::<_, i64>(1)?,
                credit: row.get(2)?,
                calls: row.get::<_, i64>(3)?,
            })
        })?;
        for row in rows.flatten() {
            model_list.push(row);
        }
        Ok(())
    })?;

    Ok(UsageOverview {
        days: day_map.into_values().collect(),
        by_app: app_map.into_values().collect(),
        by_model: model_list,
        today: today_totals,
        total,
        scanned_at: db
            .kv_get(LAST_SCAN_KV)
            .ok()
            .flatten()
            .and_then(|text| text.parse().ok()),
        coverage: coverage(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_has_one_entry_per_platform() {
        let platforms: Vec<_> = coverage().into_iter().map(|c| c.platform).collect();
        assert_eq!(
            platforms,
            [
                "workbuddy",
                "workbuddy_intl",
                "codebuddy",
                "zcode",
                "qoder",
                "trae",
                "doubao"
            ]
        );
    }

    fn roots(dir: &std::path::Path) -> Roots {
        Roots::at(dir.join("home"), dir.join("appdata"), dir.join("local"))
    }

    fn db(dir: &std::path::Path) -> Db {
        Db::open_at(&dir.join("app.db")).unwrap()
    }

    fn wb_line(id: &str, ts: i64) -> String {
        format!(
            r#"{{"sessionId":"s1","timestamp":{ts},"providerData":{{"traceId":"{id}","requestModelName":"Deepseek-V4.1-Flash","usage":{{"inputTokens":100,"outputTokens":20,"totalTokens":120}},"rawUsage":{{"credit":0.5}}}}}}"#
        )
    }

    #[test]
    fn workbuddy_jsonl_is_incremental_and_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = db(dir.path());
        let roots = roots(dir.path());
        let file = roots
            .workbuddy(true)
            .join("projects")
            .join("p1")
            .join("s1.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let now = crate::app::timeutil::now_ms();
        std::fs::write(
            &file,
            format!("{}\n{}\n", wb_line("a", now), wb_line("b", now)),
        )
        .unwrap();

        let first = scan_all(&db, &roots).unwrap();
        assert_eq!(first.inserted, 2, "首轮应写入两条：{:?}", first.errors);

        let second = scan_all(&db, &roots).unwrap();
        assert_eq!(second.inserted, 0, "重扫不应重复计数");

        // 追加一行：只补新增的那条
        let mut text = std::fs::read_to_string(&file).unwrap();
        text.push_str(&format!("{}\n", wb_line("c", now)));
        std::fs::write(&file, text).unwrap();
        let third = scan_all(&db, &roots).unwrap();
        assert_eq!(third.inserted, 1);
    }

    #[test]
    fn codebuddy_index_is_read_once_per_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = db(dir.path());
        let roots = roots(dir.path());
        let index = roots
            .codebuddy()
            .join("uid1")
            .join("CodeBuddyIDE")
            .join("uid1")
            .join("history")
            .join("ws1")
            .join("index.json");
        std::fs::create_dir_all(index.parent().unwrap()).unwrap();
        let now = crate::app::timeutil::now_ms();
        std::fs::write(
            &index,
            format!(
                r#"{{"requests":[{{"id":"r1","startedAt":{now},"usage":{{"inputTokens":50,"outputTokens":10,"totalTokens":60,"credit":0.2}}}}]}}"#
            ),
        )
        .unwrap();

        assert_eq!(scan_all(&db, &roots).unwrap().inserted, 1);
        assert_eq!(
            scan_all(&db, &roots).unwrap().inserted,
            0,
            "文件没变不重复解析"
        );
    }

    /// 只读本机真实客户端目录，写临时库。默认 ignored，手动跑：
    /// `cargo test --lib --features ts real_machine_scan -- --ignored --nocapture`
    #[test]
    #[ignore = "读本机真实客户端目录，仅手动运行"]
    fn real_machine_scan_smoke() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let report = scan_all(&db, &Roots::system()).unwrap();
        let overview = overview(&db, DayWindow::last_days(30)).unwrap();
        println!(
            "扫描={} 新增={} 跳过={:?} 错误={:?}",
            report.scanned, report.inserted, report.skipped, report.errors
        );
        println!(
            "今日={:?} 区间={:?} 平台={} 模型={}",
            overview.today,
            overview.total,
            overview.by_app.len(),
            overview.by_model.len()
        );
        assert!(report.scanned > 0, "本机应能读到至少一条用量");
    }

    #[test]
    fn overview_aggregates_today_by_app_and_model() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = db(dir.path());
        let roots = roots(dir.path());
        let file = roots
            .workbuddy(true)
            .join("projects")
            .join("p1")
            .join("s1.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let now = crate::app::timeutil::now_ms();
        std::fs::write(&file, format!("{}\n", wb_line("a", now))).unwrap();
        scan_all(&db, &roots).unwrap();

        let report = overview(&db, DayWindow::last_days(7)).unwrap();
        assert_eq!(report.today.calls, 1);
        assert_eq!(report.today.tokens, 120);
        assert!((report.today.credit - 0.5).abs() < 1e-9);
        assert_eq!(report.by_app.len(), 1);
        assert_eq!(report.by_model[0].model, "Deepseek-V4.1-Flash");
        // 区间内每天都要有一根柱子：无记录的天补 0，否则图表会把稀疏数据连成斜线
        assert_eq!(report.days.len(), 7, "7 天区间应铺满 7 天");
        assert_eq!(report.days[6].calls, 1, "今天落在最后一根柱子上");
        assert_eq!(report.days[6].tokens, 120);
        assert!(
            report.days[..6].iter().all(|day| day.calls == 0),
            "历史空天补 0"
        );
    }
}
