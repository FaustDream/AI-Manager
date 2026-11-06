//! 每日活跃：成长中心（`/profile/growth-center`）的活跃判定 + 「你好」补齐。
//!
//! 接口来自 Web 端 bundle 实测（2026-11-06，真实账号只读验证）：
//!   - `GET  /v2/activity/growth/tasks`        任务列表（进度 + 领取状态）
//!   - `GET  /activity/growth/heatmap`         每月活跃地图（`cells[].score`）
//!   - `GET  /activity/growth/streak`          连续活跃天数
//!   - `POST https://copilot.tencent.com/v2/chat/completions`  只接受 `stream: true`
//!
//! 活跃口径：heatmap 里**今天**的 `score > 0` 即算当日活跃。任务列表只用来回答
//! 「还有哪些没完成」，本模块**绝不代领任务奖励**——那属于用户自己的动作。
//!
//! 补活跃只做一件事：用客户端内置免费模型 `hy4free` 发一条「你好」。同一天只发一次，
//! 标记落在 kv；发完必须回读活跃地图，没计入就如实说「尚未计入」，不谎报成功。

use super::Service;
use crate::app::accounts::credential::Credential;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::providers::workbuddy::WorkBuddyProvider;
use serde::Serialize;
use serde_json::{json, Value};

/// 客户端内置免费模型（取自 `projects/*.jsonl` 的 `providerData.requestModelId`）。
/// 写成 `hy4-free` 会被平台判成「模型不存在」（code 11102）。
const HELLO_MODEL: &str = "hy4free";
const HELLO_TEXT: &str = "你好";
/// 「今天是否已补过活跃」的幂等标记，形如 `{ "iid|2026-11-06": ts }`。
const SENT_KV: &str = "activity.daily_hello.sent";
/// 签到后自动补活跃的总开关（kv bool，默认关）。
const HELLO_SWITCH_KV: &str = "activity.daily_hello.enabled";

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct DailyTask {
    pub code: String,
    pub title: String,
    /// `claimed` / `accepted` / …… 平台原值，界面只做展示
    pub status: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub current: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub target: i64,
}

/// 当日活跃明细：活跃地图状态 + 未完成任务 + 连续天数。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct DailyActive {
    pub date: String,
    /// 活跃地图里今天的 `score > 0`
    pub active: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub score: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub streak_days: i64,
    /// 距下一档连续奖励还差几天
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub next_tier_remaining: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub month_active_days: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub month_total_days: i64,
    /// 今天是否已经由本工具补过活跃
    pub hello_sent: bool,
    /// 未完成的任务（已完成的不返回：18 条里绝大多数早已 claimed）
    pub pending_tasks: Vec<DailyTask>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_tasks: i64,
    pub message: String,
}

/// 只读快照与「补活跃」共用同一段解析，避免两处口径漂移。
impl Service {
    /// 当天活跃明细（只读）。
    pub async fn daily_status(&self, iid: &str) -> Result<DailyActive> {
        self.daily_snapshot(iid).await
    }

    /// 补一次活跃：发一条「你好」，再回读活跃地图确认是否真的计入。
    pub async fn daily_hello(&self, iid: &str) -> Result<DailyActive> {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        // 幂等：重复发只是刷接口，还可能被平台判成异常流量
        if self.hello_sent(iid, &today) {
            let mut snapshot = self.daily_snapshot(iid).await?;
            snapshot.message = "今天已经补过活跃，不重复发送".into();
            return Ok(snapshot);
        }

        let (_, cred) = self.load(iid).await?;
        self.send_hello(&cred).await?;
        self.mark_hello(iid, &today)?;

        let mut snapshot = self.daily_snapshot(iid).await?;
        if snapshot.active {
            snapshot.message =
                format!("已发送「你好」，今日活跃已完成（score {}）", snapshot.score);
        } else {
            // 发出去了但地图没动：可能是统计延迟，也可能这条请求没被计入。
            // 两种都不该说成功——界面留一个「再看一次」的入口即可。
            snapshot.message = "已发送「你好」，但活跃地图暂未计入；稍后点「刷新」确认".into();
        }
        Ok(snapshot)
    }

    /// 签到之后统一补一次活跃（开关 `activity.daily_hello.enabled`，默认关）。
    ///
    /// 只返回「真的做了什么」：开关关着、今天都补过了、没有可补的账号，一律 None——
    /// 这类轮询每天都会跑，静默才不会把执行日志刷成心跳。
    pub async fn daily_sweep(&self) -> Option<String> {
        if !self.hello_enabled() {
            return None;
        }
        let list = crate::app::runtime::blocking({
            let db = self.db.clone();
            move || crate::app::accounts::repo::list(&db)
        })
        .await
        .ok()
        .and_then(|result| result.ok())
        .unwrap_or_default();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        let mut sent = 0;
        let mut failed = 0;
        let mut errors: Vec<String> = Vec::new();
        for account in list {
            if account.platform != "workbuddy"
                || crate::app::accounts::is_intl_account(&account.iid)
                || !account.has_secret
                || !account.enabled
            {
                continue;
            }
            // 幂等标记先看：已经补过的账号连快照都不用拉
            if self.hello_sent(&account.iid, &today) {
                continue;
            }
            match self.daily_hello(&account.iid).await {
                Ok(_) => sent += 1,
                Err(e) => {
                    failed += 1;
                    errors.push(format!("{}：{}", account.display_name(), e.message));
                }
            }
        }
        if sent == 0 && failed == 0 {
            return None;
        }
        let tail = if errors.is_empty() {
            String::new()
        } else {
            format!(" | 失败明细: {}", errors.join("；"))
        };
        Some(format!(
            "每日活跃：补发 {sent} 个账号 · 失败 {failed}{tail}"
        ))
    }

    fn hello_enabled(&self) -> bool {
        self.db
            .kv_get(HELLO_SWITCH_KV)
            .ok()
            .flatten()
            .and_then(|value| serde_json::from_str::<bool>(&value).ok())
            .unwrap_or(false)
    }

    async fn daily_snapshot(&self, iid: &str) -> Result<DailyActive> {
        let (view, cred) = self.load(iid).await?;
        if view.platform != "workbuddy" || crate::app::accounts::is_intl_account(iid) {
            return Err(AppError::new(
                ErrorKind::Unsupported,
                "每日活跃只支持 WorkBuddy 国内版",
            ));
        }
        let provider = WorkBuddyProvider::new(false);
        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let month = now.format("%Y-%m").to_string();

        // 活跃地图：一次拿全年 365 格，本地按当月/今天筛。传 `month` 平台不认（实测返回全年），
        // 所以参数只是兜底，不能依赖它。
        let heat = provider
            .growth_heatmap(&cred, self.http.as_ref(), &month)
            .await?;
        let cells = heat
            .pointer("/data/cells")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut score = 0;
        let mut month_active_days = 0;
        for cell in &cells {
            let date = cell.get("date").and_then(|v| v.as_str()).unwrap_or("");
            let value = cell.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
            if date == today {
                score = value;
            }
            if date.starts_with(&month) && value > 0 {
                month_active_days += 1;
            }
        }

        // 连续天数与任务都只是「补充信息」：挂了不该让整张卡变成错误
        let streak = provider
            .growth_streak(&cred, self.http.as_ref())
            .await
            .unwrap_or(Value::Null);
        let streak_days = streak
            .pointer("/data/streak/days")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let next_tier_remaining = streak
            .pointer("/data/streak/next_tier_remaining")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let month_total_days = streak
            .pointer("/data/streak/month_total_days")
            .and_then(|v| v.as_i64())
            .unwrap_or(month_active_days);

        let tasks = provider
            .growth_tasks(&cred, self.http.as_ref())
            .await
            .unwrap_or(Value::Null);
        let list = tasks
            .pointer("/data/tasks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut pending = Vec::new();
        for task in &list {
            let status = task
                .get("accept_status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if status == "claimed" {
                continue;
            }
            pending.push(DailyTask {
                code: task
                    .get("task_code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                title: task
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                status: status.to_string(),
                current: task
                    .pointer("/progress/current")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                target: task
                    .pointer("/progress/target")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
            });
        }

        let active = score > 0;
        Ok(DailyActive {
            date: today.clone(),
            active,
            score,
            streak_days,
            next_tier_remaining,
            month_active_days,
            month_total_days,
            hello_sent: self.hello_sent(iid, &today),
            total_tasks: list.len() as i64,
            pending_tasks: pending,
            message: if active {
                format!("今日活跃已完成（score {score}）")
            } else {
                "今日还没有活跃记录".into()
            },
        })
    }

    /// 发一条「你好」。平台只支持流式，非流式会被明确拒绝（code 11101）。
    async fn send_hello(&self, cred: &Credential) -> Result<()> {
        let body = json!({
            "model": HELLO_MODEL,
            "messages": [{ "role": "user", "content": HELLO_TEXT }],
            "stream": true
        });
        let text = WorkBuddyProvider::new(false)
            .core_text(
                cred,
                self.http.as_ref(),
                "/v2/chat/completions",
                Some(&body),
            )
            .await?;
        if !text.contains("data:") {
            return Err(AppError::new(
                ErrorKind::Provider,
                "对话接口没有返回流式内容（可能未计入活跃）",
            ));
        }
        Ok(())
    }

    fn hello_sent(&self, iid: &str, date: &str) -> bool {
        self.sent_map().contains_key(&format!("{iid}|{date}"))
    }

    fn mark_hello(&self, iid: &str, date: &str) -> Result<()> {
        let mut map = self.sent_map();
        // 只保留当天的记录：这是幂等标记，不是历史账本
        map.retain(|key, _| key.ends_with(date));
        map.insert(
            format!("{iid}|{date}"),
            json!(chrono::Local::now().timestamp_millis()),
        );
        self.db.kv_set(SENT_KV, &Value::Object(map).to_string())
    }

    fn sent_map(&self) -> serde_json::Map<String, Value> {
        self.db
            .kv_get(SENT_KV)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    }
}
