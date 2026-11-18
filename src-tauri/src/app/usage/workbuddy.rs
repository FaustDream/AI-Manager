//! WorkBuddy 国内版/国际版用量：只读 projects/*.jsonl，按文件偏移增量。
use super::{insert_many, Record, SourceCoverage, UsageScanReport};
use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

const WB_OFFSETS_KV: &str = "usage.wb_offsets";

pub(super) fn coverage() -> Vec<SourceCoverage> {
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
            note: "projects/*.jsonl 的 providerData.usage + rawUsage.credit".into(),
        },
    ]
}
// ---------------------------------------------------------------- WorkBuddy

pub(super) fn scan_workbuddy(db: &Db, roots: &Roots, report: &mut UsageScanReport) {
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
