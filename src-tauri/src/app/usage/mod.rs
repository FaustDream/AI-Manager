//! 本地用量采集：**只读**客户端落盘文件，不代理、不注入、不改任何配置。
//!
//! 数据来源（2026-11-06 实测）：
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
use crate::app::error::{AppError, ErrorKind, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

const WB_OFFSETS_KV: &str = "usage.wb_offsets";
const CB_FILES_KV: &str = "usage.cb_files";
const ZC_CURSOR_KV: &str = "usage.zcode_cursor";
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
    vec![
        SourceCoverage {
            platform: "workbuddy".into(),
            label: "WorkBuddy 国内版".into(),
            level: "full".into(),
            note: "projects/*.jsonl 的 providerData.usage + rawUsage.credit".into(),
        },
        SourceCoverage {
            platform: "workbuddy_intl".into(),
            label: "WorkBuddy 国际版".into(),
            level: "full".into(),
            note: "同上".into(),
        },
        SourceCoverage {
            platform: "codebuddy".into(),
            label: "CodeBuddy（IDE / IDEA 插件）".into(),
            level: "full".into(),
            note:
                "history/*/index.json 的 requests[].usage；按用户轮次聚合（一轮可能多次模型调用）"
                    .into(),
        },
        SourceCoverage {
            platform: "zcode".into(),
            label: "ZCode".into(),
            level: "tokens".into(),
            note: "model_usage 表：有 token，无扣费".into(),
        },
        SourceCoverage {
            platform: "qoder".into(),
            label: "Qoder".into(),
            level: "context".into(),
            note: "只有上下文占用快照，没有逐次计费".into(),
        },
        SourceCoverage {
            platform: "trae".into(),
            label: "Trae".into(),
            level: "none".into(),
            note: "本地 database.db 为加密格式，明文读不到".into(),
        },
    ]
}

// ---------------------------------------------------------------- 采集入口

pub fn scan_all(db: &Db, roots: &Roots) -> Result<UsageScanReport> {
    let started = std::time::Instant::now();
    let mut report = UsageScanReport::default();
    scan_workbuddy(db, roots, &mut report);
    scan_codebuddy(db, roots, &mut report);
    scan_zcode(db, roots, &mut report);
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

// ---------------------------------------------------------------- WorkBuddy

fn scan_workbuddy(db: &Db, roots: &Roots, report: &mut UsageScanReport) {
    let mut offsets: BTreeMap<String, u64> = db
        .kv_get(WB_OFFSETS_KV)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    for (intl, platform) in [(false, "workbuddy"), (true, "workbuddy_intl")] {
        let dir = roots.workbuddy(intl).join("projects");
        let Ok(projects) = std::fs::read_dir(&dir) else {
            report
                .skipped
                .push(format!("{platform}：没有 projects 目录"));
            continue;
        };
        for project in projects.flatten() {
            let Ok(files) = std::fs::read_dir(project.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let key = path.to_string_lossy().to_string();
                let offset = offsets.get(&key).copied().unwrap_or(0);
                match read_jsonl_usage(&path, offset, platform) {
                    Ok((records, next)) => {
                        insert_many(db, &records, report);
                        offsets.insert(key, next);
                    }
                    Err(e) => report.errors.push(format!("{key}：{}", e.message)),
                }
            }
        }
    }
    if let Ok(text) = serde_json::to_string(&offsets) {
        let _ = db.kv_set(WB_OFFSETS_KV, &text);
    }
}

fn read_jsonl_usage(path: &Path, offset: u64, platform: &str) -> Result<(Vec<Record>, u64)> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("打开会话文件失败：{e}")))?;
    // 文件被截断/重建：从头再来
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = if offset > len { 0 } else { offset };
    file.seek(SeekFrom::Start(start))
        .map_err(|e| AppError::new(ErrorKind::Io, format!("定位会话文件失败：{e}")))?;

    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut consumed = start;
    let mut records = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("读取会话文件失败：{e}")))?;
        if read == 0 {
            break;
        }
        // 半行不消费：下次从行首重读，避免把一条记录切成两半
        if !line.ends_with('\n') {
            break;
        }
        consumed += read as u64;
        if let Some(record) = parse_wb_line(&line, platform) {
            records.push(record);
        }
    }
    Ok((records, consumed))
}

fn parse_wb_line(line: &str, platform: &str) -> Option<Record> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let provider = value.get("providerData")?;
    let usage = provider.get("usage")?.as_object()?;
    let num = |key: &str| {
        usage
            .get(key)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0)
    };
    let (prompt, completion, total) = (num("inputTokens"), num("outputTokens"), num("totalTokens"));
    if total == 0 && prompt == 0 && completion == 0 {
        return None;
    }
    let text = |key: &str| {
        provider
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let session = value
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    // `traceId` 是「一轮对话」级别的（一轮可能几十次模型调用），
    // 拿它当唯一键会把整轮压成一条、少算几十倍。`messageId` 才是逐次唯一的。
    let id = if let Some(message) = text("messageId") {
        format!("{session}:{message}")
    } else {
        let trace = text("traceId")?;
        let seq = provider
            .get("stepSeq")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        format!("{session}:{trace}:{seq}")
    };
    let credit = provider
        .get("rawUsage")
        .and_then(|v| v.get("credit"))
        .and_then(|v| v.as_f64());
    Some(Record {
        record_id: format!("wb:{id}"),
        platform: platform.to_string(),
        model: text("requestModelName")
            .or_else(|| text("model"))
            .unwrap_or_else(|| "unknown".into()),
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: total.max(prompt + completion),
        credit,
        created_at: value
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(crate::app::timeutil::now_ms),
        source: "jsonl".into(),
    })
}

// ---------------------------------------------------------------- CodeBuddy

fn scan_codebuddy(db: &Db, roots: &Roots, report: &mut UsageScanReport) {
    let mut seen: BTreeMap<String, String> = db
        .kv_get(CB_FILES_KV)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    let base = roots.codebuddy();
    let Ok(uids) = std::fs::read_dir(&base) else {
        report.skipped.push("codebuddy：没有数据目录".into());
        return;
    };
    for uid_dir in uids.flatten() {
        let Ok(platforms) = std::fs::read_dir(uid_dir.path()) else {
            continue;
        };
        for platform_dir in platforms.flatten() {
            let platform = platform_dir.file_name().to_string_lossy().to_string();
            if platform != "CodeBuddyIDE" && platform != "JetBrains" {
                continue;
            }
            let Ok(accounts) = std::fs::read_dir(platform_dir.path()) else {
                continue;
            };
            for account in accounts.flatten() {
                let history = account.path().join("history");
                let mut indexes = Vec::new();
                // index.json 可能在 `history/<工作区>/` 也可能在 `history/<工作区>/<会话>/`
                collect_index_files(&history, 0, &mut indexes);
                for index in indexes {
                    let Ok(meta) = std::fs::metadata(&index) else {
                        continue;
                    };
                    let key = index.to_string_lossy().to_string();
                    let stamp = format!(
                        "{}:{}",
                        meta.modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_millis())
                            .unwrap_or(0),
                        meta.len()
                    );
                    if seen.get(&key) == Some(&stamp) {
                        continue;
                    }
                    match read_codebuddy_index(&index) {
                        Ok(records) => {
                            insert_many(db, &records, report);
                            seen.insert(key, stamp);
                        }
                        Err(e) => report.errors.push(format!("{key}：{}", e.message)),
                    }
                }
            }
        }
    }
    if let Ok(text) = serde_json::to_string(&seen) {
        let _ = db.kv_set(CB_FILES_KV, &text);
    }
}

/// 有界递归：只找 `index.json`，最多三层（工作区 / 会话），不扫全盘。
fn collect_index_files(dir: &Path, depth: usize, out: &mut Vec<std::path::PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_index_files(&path, depth + 1, out);
        } else if path.file_name().and_then(|n| n.to_str()) == Some("index.json") {
            out.push(path);
        }
    }
}

fn read_codebuddy_index(path: &Path) -> Result<Vec<Record>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("读取会话索引失败：{e}")))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("会话索引不是 JSON：{e}")))?;
    let mut out = Vec::new();
    let Some(requests) = value.get("requests").and_then(|v| v.as_array()) else {
        return Ok(out);
    };
    for request in requests {
        let Some(usage) = request.get("usage").and_then(|v| v.as_object()) else {
            continue;
        };
        let num = |key: &str| {
            usage
                .get(key)
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
                .unwrap_or(0)
        };
        let (prompt, completion, total) =
            (num("inputTokens"), num("outputTokens"), num("totalTokens"));
        if total == 0 && prompt == 0 && completion == 0 {
            continue;
        }
        let Some(id) = request.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        out.push(Record {
            record_id: format!("cb:{id}"),
            platform: "codebuddy".into(),
            model: request
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: total.max(prompt + completion),
            credit: usage.get("credit").and_then(|v| v.as_f64()),
            created_at: request
                .get("startedAt")
                .map(|v| {
                    crate::app::timeutil::parse_ts(v).unwrap_or_else(crate::app::timeutil::now_ms)
                })
                .unwrap_or_else(crate::app::timeutil::now_ms),
            source: "index_json".into(),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- ZCode

fn scan_zcode(db: &Db, roots: &Roots, report: &mut UsageScanReport) {
    let path = roots.zcode().join("cli").join("db").join("db.sqlite");
    if !path.exists() {
        report.skipped.push("zcode：没有 db.sqlite".into());
        return;
    }
    let cursor: i64 = db
        .kv_get(ZC_CURSOR_KV)
        .ok()
        .flatten()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0);
    let conn = match rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => conn,
        Err(e) => {
            report.errors.push(format!("zcode 打开失败：{e}"));
            return;
        }
    };
    let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
    let mut stmt = match conn.prepare(
        "SELECT id, model_id, input_tokens, output_tokens,
                COALESCE(NULLIF(computed_total_tokens,0), provider_total_tokens, 0),
                started_at
         FROM model_usage
         WHERE status='completed' AND started_at > ?1
         ORDER BY started_at LIMIT 5000",
    ) {
        Ok(stmt) => stmt,
        Err(e) => {
            report.errors.push(format!("zcode 查询失败：{e}"));
            return;
        }
    };
    let rows = stmt.query_map([cursor], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    });
    let Ok(rows) = rows else {
        report.errors.push("zcode 读取失败".into());
        return;
    };
    let mut records = Vec::new();
    let mut max_started = cursor;
    for row in rows.flatten() {
        let (id, model, prompt, completion, total, started_at) = row;
        max_started = max_started.max(started_at);
        if total == 0 && prompt == 0 && completion == 0 {
            continue;
        }
        records.push(Record {
            record_id: format!("zc:{id}"),
            platform: "zcode".into(),
            model,
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: total.max(prompt + completion),
            credit: None,
            created_at: started_at,
            source: "sqlite".into(),
        });
    }
    insert_many(db, &records, report);
    if max_started > cursor {
        let _ = db.kv_set(ZC_CURSOR_KV, &max_started.to_string());
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

pub fn overview(db: &Db, days: i64) -> Result<UsageOverview> {
    let days = days.clamp(1, 90);
    let today = chrono::Local::now().date_naive();
    let since = (today - chrono::Duration::days(days - 1))
        .and_hms_opt(0, 0, 0)
        .and_then(|at| at.and_local_timezone(chrono::Local).single())
        .map(|at| at.timestamp_millis())
        .unwrap_or(0);

    let mut day_map: BTreeMap<String, UsageDay> = BTreeMap::new();
    // 先把区间内每一天都铺成 0 值，图表才有完整的日期轴。
    // 只返回「有记录的天」会让相邻两点之间凭空连成斜线，看着像每天都在匀速上涨。
    let mut cursor = today - chrono::Duration::days(days - 1);
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
        if cursor >= today {
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
             FROM token_usage_records WHERE created_at >= ?1
             GROUP BY d, platform ORDER BY d",
        )?;
        let rows = stmt.query_map([since], |row| {
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
             FROM token_usage_records WHERE created_at >= ?1
             GROUP BY model ORDER BY SUM(total_tokens) DESC LIMIT 20",
        )?;
        let rows = stmt.query_map([since], |row| {
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
        let overview = overview(&db, 30).unwrap();
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

        let report = overview(&db, 7).unwrap();
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
