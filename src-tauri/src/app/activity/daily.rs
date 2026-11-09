//! 每日活跃：成长中心（`/profile/growth-center`）的活跃判定，只读。
//!
//! 接口来自 Web 端 bundle 实测（2026-11-06，真实账号只读验证）：
//!   - `GET  /v2/activity/growth/tasks`        任务列表（进度 + 领取状态）
//!   - `GET  /activity/growth/heatmap`         每月活跃地图（`cells[].score`）
//!   - `GET  /activity/growth/streak`          连续活跃天数
//!
//! 活跃口径：heatmap 里**今天**的 `score > 0` 即算当日活跃。任务列表只用来回答
//! 「还有哪些没完成」，本模块**绝不代领任务奖励**——那属于用户自己的动作。
//!
//! （曾经有过的「发一条『你好』补活跃」已移除：实测平台不计入活跃地图。）

use super::Service;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::providers::workbuddy::WorkBuddyProvider;
use serde::Serialize;
use serde_json::Value;

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
    /// 未完成的任务（已完成的不返回：18 条里绝大多数早已 claimed）
    pub pending_tasks: Vec<DailyTask>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_tasks: i64,
    pub message: String,
}

impl Service {
    /// 当天活跃明细（只读）。
    pub async fn daily_status(&self, iid: &str) -> Result<DailyActive> {
        self.daily_snapshot(iid).await
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
            date: today,
            active,
            score,
            streak_days,
            next_tier_remaining,
            month_active_days,
            month_total_days,
            total_tasks: list.len() as i64,
            pending_tasks: pending,
            message: if active {
                format!("今日活跃已完成（score {score}）")
            } else {
                "今日还没有活跃记录".into()
            },
        })
    }
}
