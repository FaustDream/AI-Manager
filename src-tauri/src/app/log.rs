//! 双落盘日志：`app.log` 给人读（带级别标记），`app.jsonl` 给机器分析。
//!
//! 单文件超 5MB 轮转，保留 3 份。级别过滤只影响 `app.log`，
//! 任务事件与性能打点（event / perf）不受门槛限制——v1 就是这么定的。

use crate::app::error::Result;
use crate::app::paths;
use chrono::Local;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "debug" => Level::Debug,
            "warn" => Level::Warn,
            "error" => Level::Error,
            _ => Level::Info,
        }
    }
}

/// 一行结构化日志，直接对应 app.jsonl 的一行。
#[derive(Debug, Serialize)]
pub struct Line {
    pub ts: String,
    pub level: Level,
    pub module: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

struct Sink {
    level: Level,
    dir: PathBuf,
    text: File,
    json: File,
}

static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Sink>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// 初始化日志。未初始化时的写入调用会被静默忽略，不影响业务。
pub fn init(level: Level) -> Result<()> {
    init_at(level, &paths::logs_dir())
}

/// 测试用：指定目录，且可重复调用覆盖上一次。
pub fn init_at(level: Level, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).ok();
    let text = open_rotating(dir.join("app.log"))?;
    let json = open_rotating(dir.join("app.jsonl"))?;
    let sink = Sink { level, dir: dir.to_path_buf(), text, json };
    *slot()
        .lock()
        .map_err(|_| crate::app::error::AppError::new("io", "日志锁被污染"))? = Some(sink);
    Ok(())
}

pub fn set_level(level: Level) {
    if let Ok(mut guard) = slot().lock() {
        if let Some(sink) = guard.as_mut() {
            sink.level = level;
        }
    }
}

pub fn write(line: Line) {
    let Ok(mut guard) = slot().lock() else { return };
    let Some(sink) = guard.as_mut() else { return };

    // 结构化行不受级别门槛限制
    let json = serde_json::to_string(&line).unwrap_or_default();
    let _ = writeln!(sink.json, "{json}");
    let _ = sink.json.flush();

    if line.level >= sink.level {
        let ts = Local::now().format("%m-%d %H:%M:%S");
        let task = line.task.as_deref().unwrap_or("-");
        let _ = writeln!(
            sink.text,
            "{ts} [{}] {} · {} · {task}",
            line.level.as_str(),
            line.module,
            line.message
        );
        let _ = sink.text.flush();
        if rotated(&sink.text).unwrap_or(false) {
            let dir = sink.dir.clone();
            sink.text = open_rotating(dir.join("app.log")).unwrap_or_else(|_| {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(std::env::temp_dir().join("ai-manager-fallback.log"))
                    .expect("兜底日志文件")
            });
        }
    }
}

pub fn log(level: Level, module: &str, message: impl Into<String>) {
    write(Line {
        ts: Local::now().to_rfc3339(),
        level,
        module: module.to_string(),
        message: message.into(),
        task: None,
    });
}

#[macro_export]
macro_rules! log_at {
    ($level:expr, $module:expr, $($arg:tt)*) => {
        $crate::app::log::log($level, $module, format!($($arg)*))
    };
}

fn rotated(file: &File) -> Result<bool> {
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len < MAX_BYTES {
        return Ok(false);
    }
    let dir = paths::logs_dir();
    // 最老的一份直接丢弃，其余依次后退
    let oldest = dir.join(format!("app.log.{}", KEEP));
    let _ = std::fs::remove_file(oldest);
    for index in (1..KEEP).rev() {
        let from = dir.join(format!("app.log.{index}"));
        let to = dir.join(format!("app.log.{}", index + 1));
        let _ = std::fs::rename(&from, &to);
    }
    let _ = std::fs::rename(dir.join("app.log"), dir.join("app.log.1"));
    Ok(true)
}

fn open_rotating(path: std::path::PathBuf) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| crate::app::error::AppError::new("io", format!("打开日志失败：{e}")))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_ordering_and_parse() {
        assert!(Level::Error > Level::Debug);
        assert_eq!(Level::from_str("warn"), Level::Warn);
        assert_eq!(Level::from_str("未知"), Level::Info);
    }

    // 日志是全局单例，同模块用例必须串行，否则互相覆盖目录
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn writes_both_files() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();
        log(Level::Info, "测试", "hello");

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        let json = std::fs::read_to_string(dir.path().join("app.jsonl")).unwrap();
        assert!(text.contains("hello"), "app.log 应有人读文本");
        assert!(json.contains("hello"), "app.jsonl 应有结构化行");
        assert!(json.trim_start().starts_with('{'), "jsonl 每行必须是 JSON 对象");
    }

    #[test]
    fn level_filter_applies_only_to_text() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Error, dir.path()).unwrap();
        log(Level::Debug, "测试", "被过滤掉的调试");

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        let json = std::fs::read_to_string(dir.path().join("app.jsonl")).unwrap();
        assert!(!text.contains("被过滤掉的调试"), "低于门槛的不进 app.log");
        assert!(json.contains("被过滤掉的调试"), "结构化行不受门槛限制");
    }
}
