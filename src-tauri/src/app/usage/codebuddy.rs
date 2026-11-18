//! CodeBuddy 用量：只读 history/*/index.json，按 mtime+size 增量。
use super::{insert_many, Record, SourceCoverage, UsageScanReport};
use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use std::collections::BTreeMap;
use std::path::Path;

const CB_FILES_KV: &str = "usage.cb_files";

pub(super) fn coverage() -> Vec<SourceCoverage> {
    vec![SourceCoverage {
        platform: "codebuddy".into(),
        label: "CodeBuddy（IDE / 插件）".into(),
        level: "full".into(),
        note: "history/*/index.json 的 requests[].usage；按用户轮次聚合（一轮可能多次模型调用）"
            .into(),
    }]
}
// ---------------------------------------------------------------- CodeBuddy

pub(super) fn scan_codebuddy(db: &Db, roots: &Roots, report: &mut UsageScanReport) {
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
