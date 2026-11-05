//! 双落盘日志：`app.log` 给人读（带级别标记），`app.jsonl` 给机器分析。
//!
//! 单文件超 5MB 轮转，保留 3 份。级别过滤只影响 `app.log`，
//! 任务事件与性能打点（event / perf）不受门槛限制——v1 就是这么定的。

use crate::app::error::{ErrorKind, Result};
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
///
/// 「执行记录」与普通业务日志共用这一份文件：带 `action` 的行表示**一次动作的完整交代**
/// （谁触发、成没成、花了多久），执行记录页只筛这一类。老 jsonl 行没有这些字段，
/// 靠 `serde(default)` 兼容——日志是历史资产，不能因为加字段就读不出来。
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Line {
    pub ts: String,
    pub level: Level,
    pub module: String,
    pub message: String,
    /// 动作键：`account_switch` / `schedule.daily_round` …；普通日志为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// 谁触发的：`user`（界面点击）/ `schedule`（定时）/ `startup`（启动自检）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// 动作结果。读操作的「失败」和写操作一样要能一眼看出来
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    /// 耗时毫秒：慢在哪一步，事后只有这个数说得清
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub ms: Option<u64>,
    /// 主体：哪个账号（`凌致`）或哪个批次（`batch-20261104-1032`）。
    /// 逐账号记录必须能一眼看出「这一行说的是谁」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// 环节：失败发生在哪一步（`登录态校验` / `提交签到` / `写客户端`…）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// 证据：可回查的原始信息（接口、状态码、业务 code、错误串）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// `ok` / `skip` / `fail` / `partial`。跳过不算失败，必须分开记
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// 重试进度 `1/3`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<String>,
    /// 批次号：批量动作的逐条记录都挂在同一个批次下
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<String>,
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
        .map_err(|_| crate::app::error::AppError::new(ErrorKind::Io, "日志锁被污染"))? = Some(sink);
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
        // 毫秒必须留：同一秒内的连续动作（切换的 5 个步骤）没有毫秒就分不出先后
        let ts = Local::now().format("%m-%d %H:%M:%S%.3f");
        // 动作行用「动作[触发方] 结果 耗时」当抬头，普通行还是模块名——
        // 执行记录要能一眼看出谁干的、成没成，不必再去 message 里找
        let head = match (line.action.as_deref(), line.trigger.as_deref()) {
            (Some(action), Some(trigger)) => format!(
                "{action}[{trigger}] {} {}ms",
                if line.ok.unwrap_or(true) {
                    "ok"
                } else {
                    "fail"
                },
                line.ms.unwrap_or(0)
            ),
            (Some(action), None) => action.to_string(),
            _ => line.module.clone(),
        };
        let _ = writeln!(
            sink.text,
            "{ts} [{}] {head} · {}{}",
            line.level.as_str(),
            line.message,
            // 结构化字段不能只活在 jsonl 里：`app.log` 是给人排障用的，
            // 证据/环节漏在这里等于没记
            structured_suffix(&line)
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
        action: None,
        trigger: None,
        ok: None,
        ms: None,
        subject: None,
        step: None,
        evidence: None,
        outcome: None,
        attempts: None,
        batch: None,
    });
}

/// 记一次动作（执行记录）：谁触发、成没成、花了多久、结果是什么。
///
/// 界面上的「执行记录」页只筛这一类（`module == "action"`）。手动点击与定时任务
/// 走同一个入口——排查问题时最怕的就是「这条到底是我点的还是它自己跑的」。
pub fn action(action: &str, trigger: &str, ok: bool, ms: u64, message: impl Into<String>) {
    write(Line {
        ts: Local::now().to_rfc3339(),
        // 失败必须可筛：Warn 在 app.log 里默认也可见
        level: if ok { Level::Info } else { Level::Warn },
        module: "action".to_string(),
        message: message.into(),
        action: Some(action.to_string()),
        trigger: Some(trigger.to_string()),
        ok: Some(ok),
        ms: Some(ms),
        subject: None,
        step: None,
        evidence: None,
        outcome: Some(if ok { "ok" } else { "fail" }.to_string()),
        attempts: None,
        batch: None,
    });
}

/// 动作结果。**跳过不是失败**：混在一起统计，「成功+跳过+失败=总计」就永远对不上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Skip,
    Fail,
    Partial,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Skip => "skip",
            Self::Fail => "fail",
            Self::Partial => "partial",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "成功",
            Self::Skip => "跳过",
            Self::Fail => "失败",
            Self::Partial => "部分成功",
        }
    }

    fn level(self) -> Level {
        match self {
            Self::Ok | Self::Skip => Level::Info,
            Self::Fail | Self::Partial => Level::Warn,
        }
    }
}

/// 一条逐项执行记录：谁、做了什么、结果、卡在哪、为什么、有什么证据、多慢。
///
/// 用 builder 而不是一个十参数函数：调用点散在签到 / 切换 / 恢复三处，
/// 位置参数一多必然写错顺序，而且看不出来漏了哪一项。
#[derive(Debug, Clone, Default)]
pub struct Item {
    action: String,
    subject: String,
    outcome: Option<Outcome>,
    step: Option<String>,
    reason: Option<String>,
    evidence: Option<String>,
    attempts: Option<String>,
    batch: Option<String>,
    note: Option<String>,
    ms: u64,
}

impl Item {
    pub fn new(action: &str, subject: &str, outcome: Outcome) -> Self {
        Self {
            action: action.to_string(),
            subject: subject.to_string(),
            outcome: Some(outcome),
            ..Default::default()
        }
    }

    /// 标记行（批次开始/结束）：不判定成败，只交代上下文，结果列显示「—」。
    pub fn marker(action: &str, subject: &str) -> Self {
        Self {
            action: action.to_string(),
            subject: subject.to_string(),
            outcome: None,
            ..Default::default()
        }
    }

    /// 自由文本，原样接在抬头后面（`开始 | 账号数=11 | 并发=3`）。
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// 环节：失败时必填，成功时写「做到哪一步」便于定位卡点。
    pub fn step(mut self, step: &str) -> Self {
        self.step = Some(step.to_string());
        self
    }

    /// 人话原因（`会话失效` / `今日已签到`）。
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// 可回查的原始证据（`POST daily-checkin → HTTP 401`、`code=10001`）。
    pub fn evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence = Some(evidence.into());
        self
    }

    /// 重试进度 `(1, 3)` → `1/3`。没有重试就别写，别让人以为重试过。
    pub fn attempts(mut self, done: u32, total: u32) -> Self {
        self.attempts = Some(format!("{done}/{total}"));
        self
    }

    pub fn batch(mut self, batch: &str) -> Self {
        self.batch = Some(batch.to_string());
        self
    }

    pub fn ms(mut self, ms: u64) -> Self {
        self.ms = ms;
        self
    }

    /// 人读的一行：`@凌致 签到 失败 | 环节=提交签到 | 原因=… | 证据=… | 重试=1/1 | 耗时=3.60s`
    fn compose(&self) -> String {
        let head = match self.outcome {
            Some(outcome) => format!("{} {} {}", self.subject, self.action, outcome.label()),
            None => format!("{} {}", self.subject, self.action),
        };
        let mut parts = vec![head];
        if let Some(note) = &self.note {
            parts.push(note.clone());
        }
        if let Some(step) = &self.step {
            parts.push(format!("环节={step}"));
        }
        if let Some(reason) = &self.reason {
            parts.push(format!("原因={reason}"));
        }
        if let Some(evidence) = &self.evidence {
            parts.push(format!("证据={evidence}"));
        }
        if let Some(attempts) = &self.attempts {
            parts.push(format!("重试={attempts}"));
        }
        parts.push(format!("耗时={}", fmt_ms(self.ms)));
        parts.join(" | ")
    }
}

/// 毫秒转人读：低于 1 秒给 ms，超过给秒。
pub fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.2}s", ms as f64 / 1000.0)
    }
}

/// 把结构化字段拼成人读后缀，**已经出现在 message 里的不重复**。
///
/// 逐项记录（`Item`）的 message 本身已经是 `主体 动作 结果 | 环节=… | 证据=…`，
/// 再拼一遍就成了复读；而 `fail()` / `ui()` 这类只写了人话的行，证据在字段里，
/// 不拼出来就等于没记。
fn structured_suffix(line: &Line) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut push = |label: &str, value: &str| {
        if !value.is_empty() && !line.message.contains(value) {
            parts.push(format!("{label}={value}"));
        }
    };
    if let Some(v) = line.subject.as_deref() {
        push("主体", v);
    }
    if let Some(v) = line.step.as_deref() {
        push("环节", v);
    }
    if let Some(v) = line.evidence.as_deref() {
        push("证据", v);
    }
    if let Some(v) = line.attempts.as_deref() {
        push("重试", v);
    }
    if let Some(v) = line.batch.as_deref() {
        push("批次", v);
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" | "))
    }
}

/// 失败日志：**出错必须能被筛出来**，所以固定 Error 级别，并且强制带证据。
///
/// 以前失败常常走 `log(Level::Info, …)`，日志页按级别筛「只看错误」时什么都看不到，
/// 用户只能一行行翻——这正是「日志不详细，找不出问题」的根因。
pub fn fail(module: &str, action: &str, message: impl Into<String>, evidence: impl Into<String>) {
    write(Line {
        ts: Local::now().to_rfc3339(),
        level: Level::Error,
        module: module.to_string(),
        message: message.into(),
        action: Some(action.to_string()),
        trigger: None,
        ok: Some(false),
        ms: None,
        subject: None,
        step: None,
        evidence: Some(evidence.into()),
        outcome: Some("fail".to_string()),
        attempts: None,
        batch: None,
    });
}

/// 界面侧日志（读失败、前端异常）。`action` 有值时带上命令名，
/// 这样日志页能直接看出「是哪个命令挂的」。
pub fn ui(level: Level, action: Option<&str>, message: impl Into<String>, evidence: Option<&str>) {
    write(Line {
        ts: Local::now().to_rfc3339(),
        level,
        module: "ui".to_string(),
        message: message.into(),
        action: action.map(|a| a.to_string()),
        trigger: Some("user".to_string()),
        ok: Some(matches!(level, Level::Info | Level::Debug)),
        ms: None,
        subject: None,
        step: None,
        evidence: evidence.map(|e| e.to_string()),
        outcome: Some(if matches!(level, Level::Info | Level::Debug) {
            "ok".to_string()
        } else {
            "fail".to_string()
        }),
        attempts: None,
        batch: None,
    });
}

/// 写一条逐项执行记录。`action` 是动作键，`subject` 是账号或批次。
pub fn item(item: Item) {
    let outcome = item.outcome;
    write(Line {
        ts: Local::now().to_rfc3339(),
        level: outcome.map(Outcome::level).unwrap_or(Level::Info),
        module: "action".to_string(),
        message: item.compose(),
        action: Some(item.action),
        // 逐项记录不单独标触发方：批次开始/结束那两行已经交代了是手动还是定时
        trigger: None,
        ok: outcome.map(|o| matches!(o, Outcome::Ok | Outcome::Skip)),
        ms: Some(item.ms),
        subject: Some(item.subject),
        step: item.step,
        evidence: item.evidence,
        outcome: outcome.map(|o| o.as_str().to_string()),
        attempts: item.attempts,
        batch: item.batch,
    });
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
                ErrorKind::Io,
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
        // 关键词要能搜到证据里：报错原文（`HTTP 401`、`os error 5`）常常只存在
        // evidence 字段，只搜 message 会让「明明记了却搜不到」。
        let hit = [
            Some(line.message.as_str()),
            line.subject.as_deref(),
            line.step.as_deref(),
            line.evidence.as_deref(),
            line.action.as_deref(),
            line.batch.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|text| text.contains(keyword));
        if !hit {
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
            // 导出的文本要和人读日志同样详细：只留 message 会让事后复盘
            // 丢掉环节和证据——那正是排障时唯一有用的部分
            text.push_str(&format!(
                "{} [{}] {} · {}{}\n",
                line.ts,
                line.level.as_str(),
                line.module,
                line.message,
                structured_suffix(line)
            ));
        }
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::app::error::AppError::new(ErrorKind::Io, format!("建目录失败：{e}"))
        })?;
    }
    std::fs::write(path, text).map_err(|e| {
        crate::app::error::AppError::new(ErrorKind::Io, format!("写导出文件失败：{e}"))
    })?;
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
        .map_err(|e| {
            crate::app::error::AppError::new(ErrorKind::Io, format!("打开日志失败：{e}"))
        })?;
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
    fn action_lines_carry_trigger_and_result() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();
        action(
            "account_switch",
            "user",
            false,
            1234,
            "切换登录失败：客户端没关掉",
        );

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(
            text.contains("account_switch[user] fail 1234ms"),
            "动作行要能一眼看出谁触发、成没成、多慢：{text}"
        );

        let rows = query(&LogFilter {
            module: Some("action".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.action.as_deref(), Some("account_switch"));
        assert_eq!(row.trigger.as_deref(), Some("user"));
        assert_eq!(row.ok, Some(false));
        assert_eq!(row.ms, Some(1234));
    }

    /// 一条逐项记录必须同时回答：谁、做了什么、结果、卡在哪、为什么、多慢。
    #[test]
    fn item_line_answers_who_what_result_why() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();

        item(
            Item::new("签到", "凌致", Outcome::Fail)
                .step("提交签到")
                .reason("会话失效")
                .evidence("HTTP=401")
                .attempts(1, 1)
                .batch("batch-x")
                .ms(3600),
        );

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(
            text.contains("凌致 签到 失败 | 环节=提交签到 | 原因=会话失效 | 证据=HTTP=401 | 重试=1/1 | 耗时=3.60s"),
            "一行要能直接判断要不要人工介入：{text}"
        );

        let rows = query(&LogFilter {
            module: Some("action".into()),
            ..Default::default()
        })
        .unwrap();
        let row = &rows[0];
        assert_eq!(row.subject.as_deref(), Some("凌致"));
        assert_eq!(row.step.as_deref(), Some("提交签到"));
        assert_eq!(row.evidence.as_deref(), Some("HTTP=401"));
        assert_eq!(row.outcome.as_deref(), Some("fail"));
        assert_eq!(row.attempts.as_deref(), Some("1/1"));
        assert_eq!(row.batch.as_deref(), Some("batch-x"));
        assert_eq!(row.ok, Some(false));
    }

    /// 跳过不是失败：统计里必须能分开，否则「成功+跳过+失败=总计」永远对不上。
    #[test]
    fn skip_is_not_fail() {
        assert_eq!(Outcome::Skip.label(), "跳过");
        assert_eq!(Outcome::Skip.as_str(), "skip");
        assert!(matches!(Outcome::Skip.level(), Level::Info));
        assert!(matches!(Outcome::Fail.level(), Level::Warn));
    }

    /// 失败必须落成 error 且带证据：按级别筛「只看错误」时不能漏，关键词也要能搜到证据。
    #[test]
    fn failure_is_error_level_and_searchable_by_evidence() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();

        fail(
            "sessions",
            "session_purge",
            "彻底删除失败：135 条全部没删掉",
            "os error 17（跨盘移动）· 目标=D:\\recycle",
        );

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(text.contains("[error]"), "失败必须是 error 级别：{text}");
        assert!(
            text.contains("证据=os error 17（跨盘移动）"),
            "证据要写进人读日志：{text}"
        );

        let errors = query(&LogFilter {
            level: Some("error".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(errors.len(), 1, "按 error 筛必须能筛到");
        assert_eq!(errors[0].outcome.as_deref(), Some("fail"));
        assert_eq!(errors[0].action.as_deref(), Some("session_purge"));

        // 报错原文常常只在 evidence 里，搜 message 搜不到
        let by_evidence = query(&LogFilter {
            keyword: Some("os error 17".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(by_evidence.len(), 1, "关键词要能搜到证据字段");
    }

    /// 逐项记录的 message 已经带了证据，人读日志不能再复读一遍。
    #[test]
    fn structured_suffix_does_not_repeat_what_message_already_says() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();

        item(
            Item::new("签到", "凌致", Outcome::Fail)
                .step("提交签到")
                .evidence("HTTP=401")
                .ms(1200),
        );

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert_eq!(
            text.matches("证据=HTTP=401").count(),
            1,
            "同一份证据不该出现两次：{text}"
        );
    }

    /// 界面失败：命令名要能看出来，错误原文要能搜到，级别必须是 error。
    #[test]
    fn ui_failure_names_the_command_and_keeps_the_error_text() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();

        ui(
            Level::Error,
            Some("session_delete"),
            "删除会话失败（session_delete）",
            Some("kind=client_running · 原文=请先关闭 WorkBuddy 客户端"),
        );

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(
            text.contains("session_delete[user] fail"),
            "抬头要能看出是哪个命令挂的：{text}"
        );
        assert!(
            text.contains("证据=kind=client_running"),
            "错误类别与原文必须落到人读日志：{text}"
        );

        let rows = query(&LogFilter {
            keyword: Some("请先关闭".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(rows.len(), 1, "报错原文要能搜到");
        assert_eq!(rows[0].outcome.as_deref(), Some("fail"));
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
