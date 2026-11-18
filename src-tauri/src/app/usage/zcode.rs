//! ZCode 用量：只读 cli/db/db.sqlite 的 model_usage，按 started_at 游标增量。
use super::{insert_many, Record, SourceCoverage, UsageScanReport};
use crate::app::clients::Roots;
use crate::app::db::Db;

const ZC_CURSOR_KV: &str = "usage.zcode_cursor";

pub(super) fn coverage() -> Vec<SourceCoverage> {
    vec![SourceCoverage {
        platform: "zcode".into(),
        label: "ZCode".into(),
        level: "tokens".into(),
        note: "model_usage 表：有 token，无扣费".into(),
    }]
}
// ---------------------------------------------------------------- ZCode

pub(super) fn scan_zcode(db: &Db, roots: &Roots, report: &mut UsageScanReport) {
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
