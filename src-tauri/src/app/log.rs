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

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
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

    /// 解析失败回落 Info——日志级别宁可多记也不静默丢。
    pub fn parse_lossy(s: &str) -> Self {
        match s {
            "debug" => Level::Debug,
            "warn" => Level::Warn,
            "error" => Level::Error,
            _ => Level::Info,
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 一行结构化日志，直接对应 app.jsonl 的一行。
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
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

/// 界面日志面板的实时通道（§6.3 的 `log:line`）：desktop 外壳订阅后转成事件。
/// 没人订阅时发送直接丢弃，落盘不受它影响。
static LIVE: OnceLock<tokio::sync::broadcast::Sender<Line>> = OnceLock::new();

pub fn subscribe_live() -> tokio::sync::broadcast::Receiver<Line> {
    let (tx, rx) = tokio::sync::broadcast::channel(256);
    match LIVE.set(tx) {
        Ok(()) => rx,
        Err(existing) => existing.subscribe(),
    }
}

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
    let sink = Sink {
        level,
        dir: dir.to_path_buf(),
        text,
        json,
    };
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
    if let Some(tx) = LIVE.get() {
        let _ = tx.send(line.clone());
    }
    let Ok(mut guard) = slot().lock() else { return };
    let Some(sink) = guard.as_mut() else { return };

    // 结构化行不受级别门槛限制
    let json = serde_json::to_string(&line).unwrap_or_default();
    let _ = writeln!(sink.json, "{json}");
    let _ = sink.json.flush();
    // jsonl 同样要轮转：托盘常驻跑数月，不轮转就是无限增长
    if rotated(&sink.json, &sink.dir, "app.jsonl").unwrap_or(false) {
        sink.json = open_rotating(sink.dir.join("app.jsonl")).unwrap_or_else(|_| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::temp_dir().join("ai-manager-fallback.jsonl"))
                .expect("兜底日志文件")
        });
    }

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
        if rotated(&sink.text, &sink.dir, "app.log").unwrap_or(false) {
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

/// 读最近 `limit` 行**人读**日志（`app.log`）。
///
/// 不做内存环形缓冲：日志页是低频操作，直接读文件尾更简单，也不占常驻内存。
pub fn tail(limit: usize) -> Vec<String> {
    let dir = slot()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|s| s.dir.clone()))
        .unwrap_or_else(paths::logs_dir);
    let Ok(text) = std::fs::read_to_string(dir.join("app.log")) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(limit);
    lines[start..].iter().map(|s| s.to_string()).collect()
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 日志查询：从 `app.jsonl` 尾部读，按级别 / 模块 / 关键词筛。
///
/// 返回 `Result` 而不是空数组：读不出来（权限、磁盘、内容不是 UTF-8）与「没有符合条件的
/// 日志」在界面上是两句完全不同的话，吞成空数组会让用户以为自己没日志。
/// 唯一例外是文件不存在——那是「还没写过日志」的正常状态，按空结果处理。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct LogFilter {
    /// 空 = 全部；给值则按级别精确匹配
    pub level: Option<String>,
    /// 模块名包含
    pub module: Option<String>,
    /// 消息包含
    pub keyword: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub limit: Option<usize>,
}

pub fn query(filter: &LogFilter) -> Result<Vec<Line>> {
    let dir = logs_dir();
    let path = dir.join("app.jsonl");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(crate::app::error::AppError::new(
                "io",
                format!("读取日志失败：{}（{e}）", path.display()),
            ))
        }
    };
    let limit = filter.limit.unwrap_or(200).min(2000);
    let mut out: Vec<Line> = Vec::new();
    // 从后往前收，收够就停：日志页只看最近的，不必解析整份文件
    for raw in text.lines().rev() {
        if let Ok(line) = serde_json::from_str::<Line>(raw) {
            if matches_filter(&line, filter) {
                out.push(line);
            }
            if out.len() >= limit {
                break;
            }
        }
    }
    out.reverse();
    Ok(out)
}

fn matches_filter(line: &Line, filter: &LogFilter) -> bool {
    if let Some(level) = filter.level.as_deref().filter(|s| !s.is_empty()) {
        if line.level.as_str() != level {
            return false;
        }
    }
    if let Some(module) = filter.module.as_deref().filter(|s| !s.is_empty()) {
        if !line.module.contains(module) {
            return false;
        }
    }
    if let Some(keyword) = filter.keyword.as_deref().filter(|s| !s.is_empty()) {
        if !line.message.contains(keyword) {
            return false;
        }
    }
    true
}

/// 导出到文件。按扩展名决定格式：`.jsonl` 给机器读，其余给人读。
/// 读日志失败会原样冒泡（导出空文件比报错更误导人）。
pub fn export(path: &Path, filter: &LogFilter) -> Result<usize> {
    let rows = query(&LogFilter {
        limit: Some(100_000),
        ..filter.clone()
    })?;
    let jsonl = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("jsonl"))
        .unwrap_or(false);
    let mut text = String::new();
    for line in &rows {
        if jsonl {
            text.push_str(&serde_json::to_string(line).unwrap_or_default());
        } else {
            text.push_str(&format!(
                "{} [{}] {} · {}\n",
                line.ts,
                line.level.as_str(),
                line.module,
                line.message
            ));
        }
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| crate::app::error::AppError::new("io", format!("建目录失败：{e}")))?;
    }
    std::fs::write(path, text)
        .map_err(|e| crate::app::error::AppError::new("io", format!("写导出文件失败：{e}")))?;
    Ok(rows.len())
}

fn logs_dir() -> PathBuf {
    slot()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|s| s.dir.clone()))
        .unwrap_or_else(crate::app::paths::logs_dir)
}

#[macro_export]
macro_rules! log_at {
    ($level:expr, $module:expr, $($arg:tt)*) => {
        $crate::app::log::log($level, $module, format!($($arg)*))
    };
}

/// 轮转用 sink 自己的目录，不是全局 logs_dir——`init_at` 允许日志落在任意
/// 目录（测试就在用），轮转错地方会把真实 data/logs/ 动了。
fn rotated(file: &File, dir: &Path, stem: &str) -> Result<bool> {
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len < MAX_BYTES {
        return Ok(false);
    }
    // 最老的一份直接丢弃，其余依次后退
    let oldest = dir.join(format!("{stem}.{}", KEEP));
    let _ = std::fs::remove_file(oldest);
    for index in (1..KEEP).rev() {
        let from = dir.join(format!("{stem}.{index}"));
        let to = dir.join(format!("{stem}.{}", index + 1));
        let _ = std::fs::rename(&from, &to);
    }
    let _ = std::fs::rename(dir.join(stem), dir.join(format!("{stem}.1")));
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
        assert_eq!(Level::parse_lossy("warn"), Level::Warn);
        assert_eq!(Level::parse_lossy("未知"), Level::Info);
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
        assert!(
            json.trim_start().starts_with('{'),
            "jsonl 每行必须是 JSON 对象"
        );
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

    #[test]
    fn query_filters_rows_and_reports_read_failure() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();
        log(Level::Info, "测试", "可查到的行");
        log(Level::Warn, "测试", "另一行");

        let rows = query(&LogFilter {
            keyword: Some("可查到".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(rows.len(), 1, "关键词要能筛");
        assert_eq!(rows[0].message, "可查到的行");

        // 内容不是 UTF-8 = 真读失败：必须冒泡成错误，不能伪装成「没有日志」
        std::fs::write(dir.path().join("app.jsonl"), [0xffu8, 0xfe, 0x00]).unwrap();
        let err = query(&LogFilter::default()).unwrap_err();
        assert_eq!(err.kind, crate::app::error::ErrorKind::Io);
        assert!(
            err.message.contains("读取日志失败"),
            "错误要说清是读失败：{}",
            err.message
        );
    }
}
