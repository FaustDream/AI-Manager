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

/// 结构化日志协议版本。新增字段默认带版本号，旧行缺省按 1 读。
pub const LOG_SCHEMA_VERSION: u8 = 1;

fn default_schema() -> u8 {
    LOG_SCHEMA_VERSION
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 一行结构化日志，直接对应 app.jsonl 的一行。
///
/// 「执行动作」与普通业务日志共用这一份文件：带 `action` 的行表示**一次动作的完整交代**
/// （谁触发、成没成、花了多久），日志页的「执行动作」视图只筛这一类。老 jsonl 行没有这些字段，
/// 靠 `serde(default)` 兼容——日志是历史资产，不能因为加字段就读不出来。
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Line {
    /// 协议版本：机器消费方先读这个字段再决定字段语义
    #[serde(default = "default_schema")]
    pub schema: u8,
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
    /// 合并次数：连续重复的同一行只写一次，次数记在这里（首行没有这个字段）。
    /// 有它才能「少输出」又不丢信息——重复 400 次和 1 次必须能区分开。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// 这一串重复里**最后一次**出现的时间；配合 `repeat` 说明「最近还在发生」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ts: Option<String>,
}

impl Line {
    /// 这一行是不是「执行动作」：动作行与逐项记录都落在 `module == "action"`。
    pub fn is_action(&self) -> bool {
        self.module == "action"
    }

    /// 旧版命令面把只读查询也回传成动作行，这些历史行不该继续占执行视图。
    pub fn is_legacy_read(&self) -> bool {
        self.action
            .as_deref()
            .is_some_and(|action| LEGACY_READ_ACTIONS.contains(&action))
    }

    /// 结果字段缺失时按 `ok` 推导，兼容尚未写 `outcome` 的老日志行。
    pub fn effective_outcome(&self) -> Option<&'static str> {
        match self.outcome.as_deref() {
            Some("ok") => Some("ok"),
            Some("skip") => Some("skip"),
            Some("fail") => Some("fail"),
            Some("partial") => Some("partial"),
            Some(_) => None,
            None => match self.ok {
                Some(true) => Some("ok"),
                Some(false) => Some("fail"),
                None => None,
            },
        }
    }
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
    // 换文件等于换会话：上一轮的待合并计数不能带进新日志
    reset_pending();
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

/// 同一条日志连续重复时的合并窗口。
///
/// 60 秒足够覆盖重试风暴、前端热更新与同一轮批量里的重复失败，又不会把
/// 「上午一次、下午一次」的同类事件并成一条——那两次之间必然夹着别的日志。
const REPEAT_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// 等待合并的重复串：首行已实时写出，这里只累计次数与最后一次时间。
struct Pending {
    key: String,
    line: Line,
    count: u32,
    last: std::time::Instant,
}

static PENDING: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();

fn pending() -> &'static Mutex<Option<Pending>> {
    PENDING.get_or_init(|| Mutex::new(None))
}

/// 判定「是不是同一条日志」：只比内容与语义，不比时间与耗时。
///
/// `ms` 一定不同、`ts` 一定不同，把它们算进去等于永不去重；`batch` 也不参与，
/// 批次号本来就写在 message 里，真正的不同批次不会撞车。
fn fingerprint(line: &Line) -> String {
    [
        line.level.as_str(),
        line.module.as_str(),
        line.message.as_str(),
        line.action.as_deref().unwrap_or(""),
        line.trigger.as_deref().unwrap_or(""),
        line.outcome.as_deref().unwrap_or(""),
        line.subject.as_deref().unwrap_or(""),
        line.step.as_deref().unwrap_or(""),
        line.evidence.as_deref().unwrap_or(""),
    ]
    .join("\u{1}")
}

/// 写一行日志。连续重复的同一行只留首行 + 一条带次数的汇总，不再逐条刷屏。
pub fn write(line: Line) {
    write_with(line, REPEAT_WINDOW);
}

fn write_with(line: Line, window: std::time::Duration) {
    let key = fingerprint(&line);
    let now = std::time::Instant::now();
    let done = {
        // 锁被 panic 污染也要继续记：日志是排障手段，
        // 不能因为某次 panic 发生在写入途中就永久静默
        let mut guard = pending().lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_mut() {
            // 窗口内的同一行：只计数，不落盘
            Some(p) if p.key == key && now.duration_since(p.last) <= window => {
                p.count += 1;
                p.last = now;
                p.line.last_ts = Some(line.ts.clone());
                return;
            }
            // 换了内容或窗口已过：把上一串的汇总补上，再开新的一串
            _ => guard.take(),
        }
    };
    if let Some(prev) = done {
        emit_repeat(prev);
    }
    emit(line.clone());
    let mut guard = pending().lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(Pending {
        key,
        line,
        count: 1,
        last: now,
    });
}

/// 把待合并的重复次数落盘。清空日志前、退出前调用，避免最后一次计数丢掉。
pub fn flush() {
    let mut guard = pending().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(prev) = guard.take() {
        emit_repeat(prev);
    }
}

/// 丢弃待合并的计数，不落盘。清空日志时用：旧计数不该写进新文件。
fn reset_pending() {
    let mut guard = pending().lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

fn emit_repeat(prev: Pending) {
    if prev.count <= 1 {
        return;
    }
    let mut line = prev.line;
    line.repeat = Some(prev.count);
    // 汇总行落在「最后一次」的位置：它是这一段重复的收尾，
    // 时间戳留在首次会让日志页把它和首行当成同一行（同 ts 同内容）而丢掉。
    if let Some(last) = line.last_ts.clone() {
        line.ts = last;
    }
    emit(line);
}

fn emit(line: Line) {
    if let Some(tx) = LIVE.get() {
        let _ = tx.send(line.clone());
    }
    // 同 pending：污染过的锁照样用，别让日志自己先哑掉
    let mut guard = slot().lock().unwrap_or_else(|e| e.into_inner());
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
            "{ts} [{}] {head} · {}{}{}",
            line.level.as_str(),
            line.message,
            // 结构化字段不能只活在 jsonl 里：`app.log` 是给人排障用的，
            // 证据/环节漏在这里等于没记
            structured_suffix(&line),
            // 合并行必须说清「重复了多少次、最近一次是什么时候」，
            // 否则读者会以为这条只发生过一次
            repeat_suffix(&line)
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

/// 合并行的尾巴：` · 重复 42 次（最近 11-06 10:22:25）`。
fn repeat_suffix(line: &Line) -> String {
    let Some(count) = line.repeat else {
        return String::new();
    };
    match line.last_ts.as_deref() {
        Some(last) => {
            let at = chrono::DateTime::parse_from_rfc3339(last)
                .map(|at| at.format("%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|_| last.to_string());
            format!(" · 重复 {count} 次（最近 {at}）")
        }
        None => format!(" · 重复 {count} 次"),
    }
}

/// 清空当前日志（人读 + 机读），并删除历史轮转文件。
/// 只动日志文件，不碰 `data/` 下的其他内容。
///
/// Windows 下打开中的日志文件不能原地截断（os error 5），所以先把 Sink 摘下、
/// 关闭句柄，再删文件并重新打开；这期间极短的写入会被丢弃，是清空动作的必然代价。
pub fn clear() -> Result<()> {
    // 待合并的计数属于旧文件，不能带到清空后的新日志里
    reset_pending();
    let old = {
        let mut guard = slot()
            .lock()
            .map_err(|_| crate::app::error::AppError::new(ErrorKind::Io, "日志锁被污染"))?;
        guard.take()
    };
    let Some(old) = old else {
        return Ok(());
    };
    let dir = old.dir.clone();
    let level = old.level;
    drop(old);
    for name in [
        "app.log",
        "app.log.1",
        "app.log.2",
        "app.log.3",
        "app.jsonl",
        "app.jsonl.1",
        "app.jsonl.2",
        "app.jsonl.3",
    ] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    let text = open_rotating(dir.join("app.log"))?;
    let json = open_rotating(dir.join("app.jsonl"))?;
    let mut guard = slot()
        .lock()
        .map_err(|_| crate::app::error::AppError::new(ErrorKind::Io, "日志锁被污染"))?;
    *guard = Some(Sink {
        level,
        dir,
        text,
        json,
    });
    Ok(())
}

pub fn log(level: Level, module: &str, message: impl Into<String>) {
    write(Line {
        schema: LOG_SCHEMA_VERSION,
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
        repeat: None,
        last_ts: None,
    });
}

/// 记一次动作（执行动作）：谁触发、成没成、花了多久、结果是什么。
///
/// 界面上的「执行动作」视图只筛这一类（`module == "action"`）。手动点击与定时任务
/// 走同一个入口——排查问题时最怕的就是「这条到底是我点的还是它自己跑的」。
pub fn action(action: &str, trigger: &str, ok: bool, ms: u64, message: impl Into<String>) {
    write(Line {
        schema: LOG_SCHEMA_VERSION,
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
        repeat: None,
        last_ts: None,
    });
}

/// 动作结果。**跳过不是失败**：混在一起统计，「成功+跳过+失败=总计」就永远对不上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Skip,
    Fail,
    /// 没做成，但原因在外部（网络不通 / 平台限流）：跟账号无关，下一轮自动重试。
    /// 单独成一档是为了让日志别把一次断网染成红色「失败」——那种红没有任何可操作性。
    Retry,
    Partial,
}

impl Outcome {
    /// 按错误类别给结果：环境性失败记「待重试」，账号/凭据类失败才是「失败」。
    pub fn of_error(kind: crate::app::error::ErrorKind) -> Self {
        if kind.is_transient() {
            Self::Retry
        } else {
            Self::Fail
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Skip => "skip",
            Self::Fail => "fail",
            Self::Retry => "retry",
            Self::Partial => "partial",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "成功",
            Self::Skip => "跳过",
            Self::Fail => "失败",
            Self::Retry => "待重试",
            Self::Partial => "部分成功",
        }
    }

    fn level(self) -> Level {
        match self {
            Self::Ok | Self::Skip => Level::Info,
            Self::Fail | Self::Retry | Self::Partial => Level::Warn,
        }
    }
}

/// 一条逐项动作记录：谁、做了什么、结果、卡在哪、为什么、有什么证据、多慢。
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
        schema: LOG_SCHEMA_VERSION,
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
        repeat: None,
        last_ts: None,
    });
}

/// 界面侧日志（读失败、前端异常）。`action` 有值时带上命令名，
/// 这样日志页能直接看出「是哪个命令挂的」。
pub fn ui(level: Level, action: Option<&str>, message: impl Into<String>, evidence: Option<&str>) {
    write(Line {
        schema: LOG_SCHEMA_VERSION,
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
        repeat: None,
        last_ts: None,
    });
}

/// 写一条逐项动作记录。`action` 是动作键，`subject` 是账号或批次。
pub fn item(item: Item) {
    let outcome = item.outcome;
    write(Line {
        schema: LOG_SCHEMA_VERSION,
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
        repeat: None,
        last_ts: None,
    });
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 日志查询：从 `app.jsonl` 尾部读，按级别 / 模块 / 关键词 / 动作维度筛。
///
/// 「日志」与「执行动作」共用这份过滤器：`action_only` 把视图收窄到动作行，
/// `outcome` / `trigger` / `subject` 则让排障时能直接定位到「谁、怎么触发、成没成」。
///
/// 返回 `Result` 而不是空数组：读不出来（权限、磁盘、内容不是 UTF-8）与「没有符合条件的
/// 日志」在界面上是两句完全不同的话，吞成空数组会让用户以为自己没日志。
/// 唯一例外是文件不存在——那是「还没写过日志」的正常状态，按空结果处理。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct LogFilter {
    /// 空 = 全部；给值则按级别精确匹配
    pub level: Option<String>,
    /// 最低级别（含）：`warn` 一次性覆盖 warn + error
    pub min_level: Option<Level>,
    /// 模块名包含
    pub module: Option<String>,
    /// 消息 / 主体 / 环节 / 证据 / 动作 / 批次 任一包含
    pub keyword: Option<String>,
    /// 主体包含
    pub subject: Option<String>,
    /// 只看动作行（`module == "action"`）
    pub action_only: Option<bool>,
    /// 动作结果精确匹配：`ok` / `skip` / `fail` / `partial`
    pub outcome: Option<String>,
    /// 触发方精确匹配：`user` / `schedule` / `startup`
    pub trigger: Option<String>,
    /// 起始时间（RFC3339，含）
    pub from: Option<String>,
    /// 结束时间（RFC3339，含）
    pub to: Option<String>,
    /// 隐藏旧版记成动作的只读命令（`ui_ready` / `account_list` …）
    pub hide_legacy: Option<bool>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub limit: Option<usize>,
}

/// 旧版命令面把只读查询也回传成动作行，这些历史行不该继续占执行视图。
/// 不改写日志文件——历史资产只做展示层过滤。
const LEGACY_READ_ACTIONS: &[&str] = &[
    "ui_ready",
    "account_list",
    "account_current",
    "account_uid",
    "account_codebuddy_targets",
    "hub_snapshot",
    "session_page",
    "session_preview",
    "session_keys",
    "session_facets",
    "session_retention",
    "session_running_clients",
    "checkin_detail",
    "daily_state",
    "log_query",
    "settings_get",
    "autostart_get",
    "schedule_list",
    "maintenance_space",
    "maintenance_trash",
];

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

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 一组同类日志：同一模块 + 动作 + 结果 + 级别。
///
/// 日志页的「异常聚合」用它回答「到底哪一类在反复出问题」，
/// 而不是让人在几千行里数同一个错误出现了几次。
#[derive(Debug, Clone, Serialize)]
pub struct LogStat {
    pub module: String,
    pub action: Option<String>,
    pub outcome: Option<String>,
    pub level: String,
    /// 合并后的行数（文件里实际有多少条记录）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rows: u64,
    /// 真实发生次数：合并行按 `repeat` 展开
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub occurrences: u64,
    pub first_ts: String,
    pub last_ts: String,
    /// 一条代表性命中内容，界面用来判断「这是不是我关心的那类」
    pub sample: String,
}

/// 聚合最近 2 万行：按「模块 + 动作 + 结果 + 级别」分组，按发生次数排序。
pub fn stats(filter: &LogFilter) -> Result<Vec<LogStat>> {
    let path = logs_dir().join("app.jsonl");
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

    let mut groups: std::collections::HashMap<(String, String, String, String), LogStat> =
        std::collections::HashMap::new();
    for raw in text.lines().rev().take(20_000) {
        let Ok(line) = serde_json::from_str::<Line>(raw) else {
            continue;
        };
        if !matches_filter(&line, filter) {
            continue;
        }
        let key = (
            line.module.clone(),
            line.action.clone().unwrap_or_default(),
            line.outcome.clone().unwrap_or_default(),
            line.level.as_str().to_string(),
        );
        let entry = groups.entry(key).or_insert_with(|| LogStat {
            module: line.module.clone(),
            action: line.action.clone(),
            outcome: line.outcome.clone(),
            level: line.level.as_str().to_string(),
            rows: 0,
            occurrences: 0,
            first_ts: line.ts.clone(),
            last_ts: line.ts.clone(),
            sample: line.message.clone(),
        });
        entry.rows += 1;
        entry.occurrences += u64::from(line.repeat.unwrap_or(1));
        if line.ts < entry.first_ts {
            entry.first_ts = line.ts.clone();
        }
        if line.ts > entry.last_ts {
            entry.last_ts = line.ts.clone();
        }
    }

    let mut out: Vec<LogStat> = groups.into_values().collect();
    // 发生次数优先，其次看最近一次：刚出问题的排前面
    out.sort_by(|a, b| {
        b.occurrences
            .cmp(&a.occurrences)
            .then_with(|| b.last_ts.cmp(&a.last_ts))
    });
    out.truncate(100);
    Ok(out)
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 现存日志的时间跨度（RFC3339，含首尾）。
///
/// 日期选择器用它限定「能选的日期」：选到范围外只会得到空列表，不如直接不给选。
/// 日志按大小轮转（`MAX_BYTES` × `KEEP`），没有固定保留天数，所以上界必须现算，
/// 不能写死「最近 N 天」——那等于对用户撒谎。
#[derive(Debug, Clone, Serialize)]
pub struct LogSpan {
    pub earliest: Option<String>,
    pub latest: Option<String>,
}

/// 读当前 `app.jsonl` 的首尾两行。文件按时间追加，所以首行就是最早、末行就是最晚；
/// 只取两端，不解析整份文件（`query` 已经够重了）。
pub fn span() -> Result<LogSpan> {
    let path = logs_dir().join("app.jsonl");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LogSpan {
                earliest: None,
                latest: None,
            })
        }
        Err(e) => {
            return Err(crate::app::error::AppError::new(
                ErrorKind::Io,
                format!("读取日志失败：{}（{e}）", path.display()),
            ))
        }
    };
    Ok(LogSpan {
        earliest: text
            .lines()
            .find_map(|raw| serde_json::from_str::<Line>(raw).ok().map(|line| line.ts)),
        latest: text
            .lines()
            .rev()
            .find_map(|raw| serde_json::from_str::<Line>(raw).ok().map(|line| line.ts)),
    })
}

fn matches_filter(line: &Line, filter: &LogFilter) -> bool {
    if let Some(level) = filter.level.as_deref().filter(|s| !s.is_empty()) {
        if line.level.as_str() != level {
            return false;
        }
    }
    if let Some(min) = filter.min_level {
        if line.level < min {
            return false;
        }
    }
    if let Some(module) = filter.module.as_deref().filter(|s| !s.is_empty()) {
        if !line.module.contains(module) {
            return false;
        }
    }
    if let Some(subject) = filter.subject.as_deref().filter(|s| !s.is_empty()) {
        if !line
            .subject
            .as_deref()
            .unwrap_or_default()
            .contains(subject)
        {
            return false;
        }
    }
    if filter.action_only.unwrap_or(false) && !line.is_action() {
        return false;
    }
    if filter.hide_legacy.unwrap_or(false) && line.is_legacy_read() {
        return false;
    }
    if let Some(outcome) = filter.outcome.as_deref().filter(|s| !s.is_empty()) {
        if line.effective_outcome() != Some(outcome) {
            return false;
        }
    }
    if let Some(trigger) = filter.trigger.as_deref().filter(|s| !s.is_empty()) {
        if line.trigger.as_deref() != Some(trigger) {
            return false;
        }
    }
    if !in_time_range(line, filter) {
        return false;
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

fn in_time_range(line: &Line, filter: &LogFilter) -> bool {
    let from = filter
        .from
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    let to = filter
        .to
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    if from.is_none() && to.is_none() {
        return true;
    }
    let Some(ts) = chrono::DateTime::parse_from_rfc3339(&line.ts).ok() else {
        return false;
    };
    if from.is_some_and(|bound| ts < bound) {
        return false;
    }
    if to.is_some_and(|bound| ts > bound) {
        return false;
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
        assert!(
            json.contains("\"schema\":1"),
            "机器消费方必须先看到协议版本：{json}"
        );
    }

    /// 清空日志：当前两份归零、历史轮转删除，不碰其他文件。
    #[test]
    fn clear_truncates_current_and_removes_rotated_logs() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();
        log(Level::Info, "测试", "清空前");
        std::fs::write(dir.path().join("app.log.1"), "old").unwrap();
        clear().unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.log")).unwrap(),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app.jsonl")).unwrap(),
            ""
        );
        assert!(!dir.path().join("app.log.1").exists());

        log(Level::Info, "测试", "清空后仍可写");
        assert!(std::fs::read_to_string(dir.path().join("app.jsonl"))
            .unwrap()
            .contains("清空后仍可写"));
    }

    /// 日期选择器的可选范围来自真实首尾行：按大小轮转没有固定保留天数，不能写死。
    #[test]
    fn span_reads_first_and_last_line() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();
        std::fs::write(
            dir.path().join("app.jsonl"),
            concat!(
                r#"{"ts":"2026-11-01T08:00:00+08:00","level":"info","module":"a","message":"一"}"#,
                "\n",
                r#"{"ts":"2026-11-03T09:30:00+08:00","level":"info","module":"a","message":"三"}"#,
                "\n",
            ),
        )
        .unwrap();

        let span = span().unwrap();
        assert_eq!(span.earliest.as_deref(), Some("2026-11-01T08:00:00+08:00"));
        assert_eq!(span.latest.as_deref(), Some("2026-11-03T09:30:00+08:00"));
    }

    #[test]
    fn span_is_empty_before_anything_is_logged() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();

        let span = span().unwrap();
        assert!(span.earliest.is_none() && span.latest.is_none());
    }

    /// 老日志行没有 schema 字段，不能因为加字段就读不出来。
    #[test]
    fn legacy_line_without_schema_reads_as_version_one() {
        let raw = r#"{"ts":"2026-01-01T00:00:00+08:00","level":"info","module":"sessions","message":"旧行"}"#;
        let line: Line = serde_json::from_str(raw).expect("旧行必须仍可解析");
        assert_eq!(line.schema, LOG_SCHEMA_VERSION);
        assert_eq!(line.message, "旧行");
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
            Some("session_purge"),
            "彻底删除会话失败（session_purge）",
            Some("kind=client_running · 原文=请先关闭 WorkBuddy 客户端"),
        );

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(
            text.contains("session_purge[user] fail"),
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

    #[test]
    fn unified_action_and_log_filters() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Debug, dir.path()).unwrap();

        // 日志 sink 是进程级的：别的模块的用例也在写日志，可能落进这个临时目录。
        // 每条都带上本用例专属标记，查询时一并筛掉外来行，计数才是确定的。
        let tag = "log-filter-case";
        let mark = |text: &str| format!("{text} {tag}");

        log(Level::Info, "sessions", mark("普通日志"));
        log(Level::Warn, "sessions", mark("警告日志"));
        // 旧版只读命令留下的动作行，默认仍在全量视图，执行视图要隐藏
        action("ui_ready", "user", true, 1, mark("界面就绪"));
        action("account_switch", "user", false, 120, mark("切换登录失败"));
        item(
            Item::new("签到", "凌致", Outcome::Skip)
                .reason(mark("今日已签到"))
                .ms(30),
        );
        fail(
            "sessions",
            "session_purge",
            mark("彻底删除失败"),
            "os error 17",
        );

        let action_view = query(&LogFilter {
            keyword: Some(tag.into()),
            action_only: Some(true),
            hide_legacy: Some(true),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(action_view.len(), 2, "动作视图只保留真实动作与逐项记录");
        assert!(action_view
            .iter()
            .all(|line| line.module == "action" && line.action.is_some()));

        let failed = query(&LogFilter {
            keyword: Some(tag.into()),
            action_only: Some(true),
            hide_legacy: Some(true),
            outcome: Some("fail".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].action.as_deref(), Some("account_switch"));

        let skipped = query(&LogFilter {
            keyword: Some(tag.into()),
            action_only: Some(true),
            outcome: Some("skip".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].subject.as_deref(), Some("凌致"));

        let scheduled = query(&LogFilter {
            keyword: Some(tag.into()),
            action_only: Some(true),
            trigger: Some("schedule".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(scheduled.is_empty(), "逐项记录不单独标触发方");

        // 只看异常要一次覆盖 warn + error，而不是只筛一个级别
        let warn_plus = query(&LogFilter {
            keyword: Some(tag.into()),
            min_level: Some(Level::Warn),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(warn_plus.len(), 3, "警告日志 + 失败动作 + 失败日志");

        let by_subject = query(&LogFilter {
            keyword: Some(tag.into()),
            subject: Some("凌致".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(by_subject.len(), 1);

        let future = query(&LogFilter {
            keyword: Some(tag.into()),
            from: Some("2099-01-01T00:00:00+08:00".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(future.is_empty(), "时间下界应过滤掉历史行");
    }

    fn info_line(module: &str, message: &str) -> Line {
        Line {
            schema: LOG_SCHEMA_VERSION,
            ts: Local::now().to_rfc3339(),
            level: Level::Info,
            module: module.into(),
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
            repeat: None,
            last_ts: None,
        }
    }

    /// 连续重复的同一行只留首行 + 一条带次数的汇总，不再逐条刷屏。
    #[test]
    fn repeated_lines_collapse_into_one_counted_summary() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Info, dir.path()).unwrap();
        // 模块名当隔离标记：sink 是进程级的，别的用例可能同时写日志
        let case = "repeat-case";
        for _ in 0..5 {
            log(Level::Info, case, "前端已就绪");
        }
        // 换一条内容：上一串的汇总在这时补写，不必等进程退出
        log(Level::Info, case, "启动体检");

        let rows = query(&LogFilter {
            module: Some(case.into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(rows.len(), 3, "首行 + 汇总 + 新行：{rows:?}");
        assert_eq!(rows[0].message, "前端已就绪");
        assert_eq!(rows[0].repeat, None, "首行实时写出，不带次数");
        assert_eq!(rows[1].message, "前端已就绪");
        assert_eq!(rows[1].repeat, Some(5), "汇总要保留总次数");
        assert!(rows[1].last_ts.is_some(), "汇总要能说出最近一次发生时间");
        assert_ne!(rows[0].ts, rows[1].ts, "汇总要落在最后一次的位置");
        assert_eq!(rows[2].message, "启动体检");
    }

    /// 内容不同就不合并：A/B/A 是三次独立事件。
    #[test]
    fn different_messages_are_never_merged() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Info, dir.path()).unwrap();
        let case = "diff-case";
        log(Level::Info, case, "A");
        log(Level::Info, case, "B");
        log(Level::Info, case, "A");

        let rows = query(&LogFilter {
            module: Some(case.into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|line| line.repeat.is_none()));
    }

    /// 异常也参与合并，但级别与证据必须保留：合并 ≠ 静默丢弃。
    #[test]
    fn repeated_errors_keep_level_and_evidence_with_count() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Info, dir.path()).unwrap();
        let case = "error-case";
        for _ in 0..3 {
            fail(
                case,
                "credit_claim_read",
                "读取积分领取记录失败",
                "code=500",
            );
        }
        flush();

        let rows = query(&LogFilter {
            module: Some(case.into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(rows.len(), 2, "首条 + 汇总：{rows:?}");
        let summary = &rows[1];
        assert_eq!(summary.level, Level::Error, "合并后仍是错误级");
        assert_eq!(summary.evidence.as_deref(), Some("code=500"), "证据不能丢");
        assert_eq!(summary.repeat, Some(3), "必须保留发生次数");
    }

    /// 窗口过期后重新计数：同一句话隔很久再出现，是两次事件，不能并成一条。
    #[test]
    fn expired_window_starts_a_new_streak() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Info, dir.path()).unwrap();
        let case = "window-case";
        write_with(info_line(case, "定时轮询"), std::time::Duration::ZERO);
        std::thread::sleep(std::time::Duration::from_millis(2));
        write_with(info_line(case, "定时轮询"), std::time::Duration::ZERO);

        let rows = query(&LogFilter {
            module: Some(case.into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(rows.len(), 2, "窗口外不合并：{rows:?}");
        assert!(rows.iter().all(|line| line.repeat.is_none()));
    }

    /// 合并计数必须能主动落盘：否则进程退出前的最后一串就只剩首行。
    #[test]
    fn flush_writes_the_pending_count() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Info, dir.path()).unwrap();
        let case = "flush-case";
        log(Level::Info, case, "重载界面");
        log(Level::Info, case, "重载界面");
        flush();

        let rows = query(&LogFilter {
            module: Some(case.into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].repeat, Some(2));
    }

    /// 合并行在 app.log 里也要说清次数，人读的那份不能只剩一句话。
    #[test]
    fn repeat_suffix_is_rendered_for_humans() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::TempDir::new().unwrap();
        init_at(Level::Info, dir.path()).unwrap();
        log(Level::Info, "ui", "心跳");
        log(Level::Info, "ui", "心跳");
        flush();

        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(text.contains("重复 2 次"), "人读文本要带次数：{text}");
    }
}
