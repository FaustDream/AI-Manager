//! WorkBuddy「猫猫旅行」活动编排。
//!
//! 协议来自 `wb_tool` 的静态端点 + 本机真实账号只读验证（2026-11-05）：
//!   - `GET  /activity/growth/buddy/travel/status`   当前状态
//!   - `GET  /activity/growth/buddy/travel/config`   地点与奖励区间
//!   - `GET  /activity/growth/buddy/travel/records`  历史记录
//!   - `POST /activity/growth/buddy/travel/claim`    领取奖励（服务端幂等）
//!   - `POST /activity/growth/buddy/travel/depart`   派出，body `{location_id}`
//!
//! **写操作默认关闭**：账号级 `travel.enabled` 缺省 false，只有用户在详情页
//! 打开并配置后，才会进入「猫猫旅行」定时轮询；手动领取/派出始终由用户点击。

pub mod store;

use crate::app::accounts::{self, AccountView};
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::log::{self, Item, Outcome};
use crate::app::net::Http;
use crate::app::providers::{self, Provider};
use crate::app::runtime::blocking;
use crate::types::ActionResult;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TravelState {
    Idle,
    Traveling,
    Arrived,
    /// 今日上限已到：不是失败，今天不会再派出
    Limit,
    Unknown,
}

impl TravelState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "待派出",
            Self::Traveling => "旅行中",
            Self::Arrived => "已回来",
            Self::Limit => "今日上限",
            Self::Unknown => "状态未知",
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TravelLocation {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: i64,
    pub code: String,
    pub name: String,
    pub description: Option<String>,
    /// status 里的本次时长
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub duration_hours: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub duration_hours_min: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub duration_hours_max: Option<i64>,
    pub reward_credit_min: Option<f64>,
    pub reward_credit_max: Option<f64>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TravelStatus {
    pub state: Option<TravelState>,
    pub location: Option<TravelLocation>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub arrive_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub depart_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub server_now: Option<i64>,
    pub reward_credit: Option<f64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub duration_hours: Option<i64>,
    pub daily_limit_reached: bool,
    pub letter: Option<String>,
    pub buddy_name: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub record_id: Option<i64>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TravelConfig {
    pub buddy_name: Option<String>,
    pub locations: Vec<TravelLocation>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TravelRecord {
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub record_id: Option<i64>,
    pub travel_date: Option<String>,
    pub location_name: Option<String>,
    pub reward_credit: Option<f64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub depart_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub arrive_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub claimed_at: Option<i64>,
    pub letter: Option<String>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TravelPrefs {
    /// 是否参与「猫猫旅行」定时轮询。缺省 false：写操作必须由用户显式打开。
    pub enabled: bool,
    /// 指定地点；None = 每次取平台地点列表的第一个
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub location_id: Option<i64>,
}

/// 本机旅行记录里的一条：派出 / 回来 / 领到积分。
/// 平台只给「已完成的旅行」列表，出发与到站时刻只有本机知道。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TravelEvent {
    /// `depart` / `arrive` / `claim`
    pub kind: String,
    /// 事件时刻（毫秒）。arrive 用平台给的到站时刻，其余用本机时钟。
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at: i64,
    pub location: String,
    /// 领到的积分；派出/回来没有数额
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub amount: Option<f64>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct ActivityOverview {
    pub status: TravelStatus,
    pub config: TravelConfig,
    pub records: Vec<TravelRecord>,
    pub prefs: TravelPrefs,
    /// 本机记录（派出/回来/领取），最新的在前。
    pub events: Vec<TravelEvent>,
    /// 非致命的查询失败（地点/记录读不到时仍给状态）。
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RoundOutcome {
    pub total: usize,
    pub claimed: usize,
    pub departed: usize,
    pub waiting: usize,
    pub failed: usize,
    pub message: String,
}

mod daily;

pub use daily::{DailyActive, DailyTask};

#[derive(Clone)]
pub struct Service {
    db: Arc<Db>,
    http: Arc<dyn Http>,
}

impl Service {
    pub fn new(db: Arc<Db>, http: Arc<dyn Http>) -> Self {
        Self { db, http }
    }

    async fn load(&self, iid: &str) -> Result<(AccountView, accounts::credential::Credential)> {
        accounts::load_credential(self.db.clone(), iid).await
    }

    fn provider_of(iid: &str) -> Result<Box<dyn Provider>> {
        providers::for_iid(iid)
            .ok_or_else(|| AppError::new(ErrorKind::Unsupported, "这个平台的账号暂不支持活动"))
    }

    /// 详情卡一次取齐：状态 + 地点 + 记录 + 账号偏好。
    pub async fn overview(&self, iid: &str) -> Result<ActivityOverview> {
        let (view, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        if !provider.travel_supported() {
            return Err(AppError::new(
                ErrorKind::Unsupported,
                format!("{} 暂不支持猫猫旅行", view.platform_label),
            ));
        }

        let mut warnings = Vec::new();
        let mut status = match provider.travel_status(&cred, self.http.as_ref()).await {
            Ok(status) => status,
            Err(e) => {
                log::fail(
                    "activity",
                    "activity_status",
                    format!("读取旅行状态失败：{}", view.display_name()),
                    e.evidence(),
                );
                return Err(e);
            }
        };
        let config = match provider.travel_config(&cred, self.http.as_ref()).await {
            Ok(config) => config,
            Err(e) => {
                warnings.push(format!("地点读取失败：{}", e.message));
                TravelConfig::default()
            }
        };
        let records = match provider.travel_records(&cred, self.http.as_ref()).await {
            Ok(records) => records,
            Err(e) => {
                warnings.push(format!("记录读取失败：{}", e.message));
                Vec::new()
            }
        };
        if status.buddy_name.is_none() {
            status.buddy_name = config.buddy_name.clone();
        }
        self.note_arrival(iid, &status).await;

        let db = self.db.clone();
        let key = iid.to_string();
        let (prefs, events) =
            blocking(move || (store::prefs_of(&db, &key), store::events_of(&db, &key)))
                .await
                .unwrap_or_default();

        Ok(ActivityOverview {
            status,
            config,
            records,
            prefs,
            events,
            warnings,
        })
    }

    /// 手动领取；写入前由 `travel_status` 判断是否 arrived，服务端再做幂等。
    pub async fn claim(&self, iid: &str, trigger: &str) -> Result<ActionResult> {
        let started = std::time::Instant::now();
        let (view, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        // 与自动路径使用同一次旅行的身份，不能用点击时刻作为去重键。
        let status = provider
            .travel_status(&cred, self.http.as_ref())
            .await
            .inspect_err(|e| {
                log::fail(
                    "activity",
                    "activity_claim",
                    "领取前查询旅行状态失败",
                    e.evidence(),
                );
            })?;
        let result = provider.travel_claim(&cred, self.http.as_ref()).await;
        if let Ok(value) = &result {
            self.record_travel_claim(iid, &status, value, &crate::app::clock::SystemClock)
                .await;
            self.note_arrival(iid, &status).await;
        }
        let ms = started.elapsed().as_millis() as u64;
        match &result {
            Ok(value) => log::action(
                "activity_claim",
                trigger,
                value.ok,
                ms,
                format!("{}：{}", view.display_name(), value.message),
            ),
            Err(e) => log::fail(
                "activity",
                "activity_claim",
                format!("领取失败：{}", view.display_name()),
                format!("{} · {}", e.evidence(), view.platform_label),
            ),
        }
        result
    }

    /// 手动派出；不传地点时用账号偏好，没配就取平台地点列表第一个。
    pub async fn depart(
        &self,
        iid: &str,
        location_id: Option<i64>,
        trigger: &str,
    ) -> Result<ActionResult> {
        let started = std::time::Instant::now();
        let (view, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        let (location_id, location_name) =
            match self.resolve_location(&*provider, &cred, location_id).await {
                Ok(pair) => pair,
                Err(e) => {
                    log::fail(
                        "activity",
                        "activity_depart",
                        format!("派出失败：{}", view.display_name()),
                        e.evidence(),
                    );
                    return Err(e);
                }
            };
        let result = provider
            .travel_depart(&cred, self.http.as_ref(), location_id)
            .await;
        let ms = started.elapsed().as_millis() as u64;
        match &result {
            Ok(value) => {
                log::action(
                    "activity_depart",
                    trigger,
                    value.ok,
                    ms,
                    format!("{}：{}", view.display_name(), value.message),
                );
                if value.ok {
                    self.note_depart(iid, &location_name).await;
                }
            }
            Err(e) => log::fail(
                "activity",
                "activity_depart",
                format!("派出失败：{}", view.display_name()),
                format!("{} · 地点={location_id}", e.evidence()),
            ),
        }
        result
    }

    pub async fn set_prefs(
        &self,
        iid: &str,
        enabled: bool,
        location_id: Option<i64>,
    ) -> Result<TravelPrefs> {
        let db = self.db.clone();
        let key = iid.to_string();
        let identity = blocking(move || accounts::repo::identity(&db, &key))
            .await??
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "账号不存在，可能已被删除"))?;
        if !Self::provider_of(iid)?.travel_supported() {
            return Err(AppError::new(
                ErrorKind::Unsupported,
                "这个平台暂不支持猫猫旅行",
            ));
        }
        let prefs = TravelPrefs {
            enabled,
            location_id,
        };
        let db = self.db.clone();
        let key = iid.to_string();
        let name = crate::app::accounts::view::mask_phone(&identity.name);
        let saved =
            blocking(move || crate::app::schedule::sync_travel_prefs(&db, &key, &name, prefs))
                .await??;
        log::action(
            "activity_pref",
            "user",
            true,
            0,
            format!(
                "{}：自动旅行{}",
                crate::app::accounts::view::mask_phone(&identity.name),
                if saved.enabled {
                    "已开启"
                } else {
                    "已关闭"
                }
            ),
        );
        Ok(saved)
    }

    /// 定时轮询：只处理「启用 + 自动旅行打开」的国内版 WorkBuddy 账号。
    pub async fn round(&self, trigger: &str) -> RoundOutcome {
        self.round_for(trigger, None).await
    }

    /// 账号任务与全局任务共用筛选和执行链，指定账号也不能绕过自动旅行开关。
    pub async fn round_for(&self, trigger: &str, account_iid: Option<&str>) -> RoundOutcome {
        let started = std::time::Instant::now();
        let db = self.db.clone();
        let accounts = match blocking(move || accounts::repo::list(&db)).await {
            Ok(Ok(list)) => list,
            Ok(Err(e)) | Err(e) => {
                let message = format!("读取账号失败：{}", e.message);
                log::fail("activity", "activity_round", &message, e.evidence());
                return RoundOutcome {
                    failed: 1,
                    message,
                    ..Default::default()
                };
            }
        };
        let db = self.db.clone();
        let prefs = blocking(move || store::all(&db)).await.unwrap_or_default();
        let targets: Vec<AccountView> = accounts
            .into_iter()
            .filter(|account| {
                account_iid.is_none_or(|iid| account.iid == iid)
                    && account.platform == "workbuddy"
                    && !accounts::is_intl_account(&account.iid)
                    && account.has_secret
                    && account.enabled
                    && prefs.get(&account.iid).map(|p| p.enabled).unwrap_or(false)
            })
            .collect();

        let total = targets.len();
        if total == 0 {
            let message = "没有启用自动旅行的账号（在账号详情里打开「自动旅行」）".to_string();
            return RoundOutcome {
                message,
                ..Default::default()
            };
        }

        let mut round = RoundOutcome {
            total,
            ..Default::default()
        };
        for account in &targets {
            let started_one = std::time::Instant::now();
            let account_prefs = prefs.get(&account.iid).cloned().unwrap_or_default();
            let step = self.run_one(account, &account_prefs).await;
            match step.outcome {
                Outcome::Ok => {
                    if step.claimed {
                        round.claimed += 1;
                    }
                    if step.departed {
                        round.departed += 1;
                    }
                }
                // 领到了但没派出：算部分成功，同时计一次失败，避免定时任务被当成全绿
                Outcome::Partial => {
                    if step.claimed {
                        round.claimed += 1;
                    }
                    if step.departed {
                        round.departed += 1;
                    }
                    round.failed += 1;
                }
                Outcome::Skip => round.waiting += 1,
                Outcome::Fail => round.failed += 1,
            }
            // 只为「有变化」或「失败」留逐项记录：旅行中、今日上限这类正常等待
            // 每小时都会出现一次，逐条记等于把日志变成心跳，反而盖住真正需要看的失败。
            if step.outcome != Outcome::Skip {
                let mut item = Item::new("猫猫旅行", &account.display_name(), step.outcome)
                    .step(&step.step)
                    .reason(step.reason)
                    .ms(started_one.elapsed().as_millis() as u64);
                if !step.evidence.is_empty() {
                    item = item.evidence(step.evidence);
                }
                log::item(item);
            }
        }

        round.message = format!(
            "猫猫旅行：{total} 个账号 · 领取 {} · 派出 {} · 等待/跳过 {} · 失败 {}",
            round.claimed, round.departed, round.waiting, round.failed
        );
        // 定时触发由调度器统一记一条；手动/托盘触发才在这里落执行记录。
        // 全轮无事可做（没有领取、没有派出、没有失败）也不记——那是心跳。
        if trigger != "schedule" && (round.claimed > 0 || round.departed > 0 || round.failed > 0) {
            log::action(
                "activity_round",
                trigger,
                round.failed == 0,
                started.elapsed().as_millis() as u64,
                round.message.clone(),
            );
        }
        round
    }

    async fn resolve_location(
        &self,
        provider: &dyn Provider,
        cred: &accounts::credential::Credential,
        wanted: Option<i64>,
    ) -> Result<(i64, String)> {
        let config = provider
            .travel_config(cred, self.http.as_ref())
            .await
            .map_err(|e| {
                AppError::new(
                    ErrorKind::Provider,
                    format!("读取地点列表失败：{}", e.message),
                )
            })?;
        let location = wanted
            .and_then(|id| config.locations.iter().find(|item| item.id == id))
            .or_else(|| config.locations.first())
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "平台没有返回可去的地点"))?;
        Ok((location.id, location.name.clone()))
    }

    async fn run_one(&self, account: &AccountView, prefs: &TravelPrefs) -> Step {
        let (view, cred) = match self.load(&account.iid).await {
            Ok(pair) => pair,
            Err(e) => return Step::fail("读取凭据", &e.message, &e.evidence()),
        };
        let provider = match Self::provider_of(&account.iid) {
            Ok(provider) => provider,
            Err(e) => return Step::fail("平台能力", &e.message, &e.evidence()),
        };
        let status = match provider.travel_status(&cred, self.http.as_ref()).await {
            Ok(status) => status,
            Err(e) => return Step::fail("查询状态", &e.message, &e.evidence()),
        };
        self.note_arrival(&account.iid, &status).await;

        let state = status.state.unwrap_or(TravelState::Unknown);
        // `daily_limit_reached` 约束的是「今天还能不能再派一趟」，不是「已到手的奖励不许领」。
        // 先判上限会把 Arrived 一起挡在门外——旅行回来了、奖励挂在账上，却永远领不到。
        // 实测平台在「已回来待领取」时仍会给 daily_limit_reached=true。
        if !matches!(state, TravelState::Arrived) && status.daily_limit_reached {
            return Step::skip("今日上限", "今日旅行上限已到", "");
        }
        match state {
            TravelState::Traveling => Step::skip(
                "旅行中",
                format!(
                    "预计 {} 回来",
                    status.arrive_at.map(fmt_ms).unwrap_or_else(|| "—".into())
                ),
                "",
            ),
            TravelState::Limit => Step::skip("今日上限", "今日旅行上限已到", ""),
            TravelState::Unknown => {
                let evidence = status
                    .location
                    .as_ref()
                    .map(|l| format!("地点={}", l.name))
                    .unwrap_or_default();
                Step::fail("查询状态", "平台返回了未知的旅行状态", &evidence)
            }
            TravelState::Idle => match self.depart_step(&*provider, &cred, prefs).await {
                Ok((name, message)) => {
                    self.note_depart(&account.iid, &name).await;
                    Step {
                        outcome: Outcome::Ok,
                        departed: true,
                        claimed: false,
                        step: "派出".into(),
                        reason: message,
                        evidence: format!("地点={name} · 平台={}", view.platform_label),
                    }
                }
                Err(e) => Step::fail("派出", &e.message, &e.evidence()),
            },
            TravelState::Arrived => {
                let claimed = match provider.travel_claim(&cred, self.http.as_ref()).await {
                    Ok(result) if result.ok => result,
                    Ok(result) => {
                        return Step::fail("领取", &result.message, "领取返回 ok=false");
                    }
                    Err(e) => return Step::fail("领取", &e.message, &e.evidence()),
                };
                self.record_travel_claim(
                    &account.iid,
                    &status,
                    &claimed,
                    &crate::app::clock::SystemClock,
                )
                .await;
                // 今天派不出去就别白试：省一次请求，也避免每小时一条「领取后派出失败」
                if status.daily_limit_reached {
                    return Step {
                        outcome: Outcome::Ok,
                        claimed: !claimed.already,
                        departed: false,
                        step: "领取".into(),
                        reason: claimed.message,
                        evidence: format!("今日不可再派出 · 平台={}", view.platform_label),
                    };
                }
                match self.depart_step(&*provider, &cred, prefs).await {
                    Ok((name, message)) => {
                        self.note_depart(&account.iid, &name).await;
                        Step {
                            outcome: Outcome::Ok,
                            claimed: !claimed.already,
                            departed: true,
                            step: "领取并派出".into(),
                            reason: format!("{} · {message}", claimed.message),
                            evidence: format!("地点={name} · 平台={}", view.platform_label),
                        }
                    }
                    Err(e) => Step {
                        outcome: Outcome::Partial,
                        claimed: !claimed.already,
                        departed: false,
                        step: "领取后派出".into(),
                        reason: format!("{}；派出失败：{}", claimed.message, e.message),
                        evidence: e.evidence(),
                    },
                }
            }
        }
    }

    /// 记「派出」。地点此刻刚确定，用本机时钟即可；重复派出由状态机天然防重
    /// （派出成功后状态就是旅行中，不会再次进入派出分支）。
    async fn note_depart(&self, iid: &str, location: &str) {
        let db = self.db.clone();
        let key = iid.to_string();
        let event = TravelEvent {
            kind: "depart".into(),
            at: crate::app::timeutil::now_ms(),
            location: location.to_string(),
            amount: None,
        };
        let _ = blocking(move || {
            store::record_event(&db, &key, event);
            Ok::<(), AppError>(())
        })
        .await;
    }

    /// 记「回来」。同一趟旅行会被 overview / 轮询 / 手动领取反复观察到，
    /// store 按平台的 `arrive_at` 去重，只落一条。
    async fn note_arrival(&self, iid: &str, status: &TravelStatus) {
        if status.state != Some(TravelState::Arrived) {
            return;
        }
        let at = status
            .arrive_at
            .filter(|at| *at > 0)
            .unwrap_or_else(crate::app::timeutil::now_ms);
        let db = self.db.clone();
        let key = iid.to_string();
        let event = TravelEvent {
            kind: "arrive".into(),
            at,
            location: status
                .location
                .as_ref()
                .map(|location| location.name.clone())
                .unwrap_or_default(),
            amount: status.reward_credit,
        };
        let _ = blocking(move || {
            store::record_event(&db, &key, event);
            Ok::<(), AppError>(())
        })
        .await;
    }

    async fn record_travel_claim(
        &self,
        iid: &str,
        status: &TravelStatus,
        result: &ActionResult,
        clock: &dyn crate::app::clock::Clock,
    ) {
        if !result.ok || result.already {
            return;
        }
        let identity = status
            .record_id
            .filter(|id| *id > 0)
            .map(|id| format!("id:{id}"))
            .or_else(|| {
                status
                    .depart_at
                    .filter(|at| *at > 0)
                    .map(|at| format!("depart:{at}"))
            })
            .or_else(|| {
                status
                    .arrive_at
                    .filter(|at| *at > 0)
                    .map(|at| format!("arrive:{at}"))
            });
        let Some(identity) = identity else {
            log::fail(
                "activity",
                "credit_claim_record",
                "旅行已领取，但缺少可去重的旅行标识",
                AppError::new(
                    ErrorKind::Provider,
                    "旅行状态缺少 record_id、depart_at 和 arrive_at",
                )
                .evidence(),
            );
            return;
        };
        let amount = result.granted.or(status.reward_credit);
        let location = status
            .location
            .as_ref()
            .map(|location| location.name.clone())
            .unwrap_or_default();
        crate::app::checkin::store::record_claim(
            self.db.clone(),
            iid,
            crate::app::checkin::store::CreditClaim {
                event_key: format!("travel:{identity}"),
                source: "travel".into(),
                // 领取接口不一定回传奖励数额：退回领取前状态里的「本次奖励」，
                // 免得记录里明明领到了却显示「未提供」。
                amount,
                description: if location.is_empty() {
                    "猫猫旅行".into()
                } else {
                    format!("猫猫旅行 · {location}")
                },
                claimed_at: clock.now().timestamp_millis(),
            },
        )
        .await;
        // 台账之外同步落一条本机旅行记录，卡片上的时间线才是完整的
        let db = self.db.clone();
        let key = iid.to_string();
        let event = TravelEvent {
            kind: "claim".into(),
            at: clock.now().timestamp_millis(),
            location,
            amount,
        };
        let _ = blocking(move || {
            store::record_event(&db, &key, event);
            Ok::<(), AppError>(())
        })
        .await;
    }

    async fn depart_step(
        &self,
        provider: &dyn Provider,
        cred: &accounts::credential::Credential,
        prefs: &TravelPrefs,
    ) -> Result<(String, String)> {
        let (location_id, name) = self
            .resolve_location(provider, cred, prefs.location_id)
            .await?;
        let result = provider
            .travel_depart(cred, self.http.as_ref(), location_id)
            .await?;
        if !result.ok {
            return Err(AppError::new(ErrorKind::Provider, result.message));
        }
        Ok((name, result.message))
    }
}

#[derive(Debug, Clone)]
struct Step {
    outcome: Outcome,
    claimed: bool,
    departed: bool,
    step: String,
    reason: String,
    evidence: String,
}

impl Step {
    fn skip(step: &str, reason: impl Into<String>, evidence: &str) -> Self {
        Self {
            outcome: Outcome::Skip,
            claimed: false,
            departed: false,
            step: step.into(),
            reason: reason.into(),
            evidence: evidence.into(),
        }
    }

    fn fail(step: &str, reason: &str, evidence: &str) -> Self {
        Self {
            outcome: Outcome::Fail,
            claimed: false,
            departed: false,
            step: step.into(),
            reason: reason.into(),
            evidence: evidence.into(),
        }
    }
}

fn fmt_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}
