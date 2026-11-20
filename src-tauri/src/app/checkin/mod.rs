//! 签到与积分的编排层：查状态 → 判幂等 → 调接口 → 记台账。
//!
//! 三条顺序不能改（`docs/phase-3-checkin.md` §4.2）：
//!   **日期检测 → 账号状态校验 → 调用签到接口**
//!
//! 幂等只有一道防线：`daily_marks`。它必须在**调接口之前**检查、
//! 在**成功之后**写入、失败**不写**。这一条错了，用户一天会被扣好几次。

pub mod store;

pub use store::{SqliteCheckin, SqliteLedger};

use crate::app::accounts::{self, AccountView};
use crate::app::clock::Clock;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::log::{self, Item, Outcome};
use crate::app::net::Http;
use crate::app::progress;
use crate::app::providers;
use crate::app::runtime::blocking;
use crate::app::window::DayWindow;
use crate::contracts::{CheckinStore, Ledger};
use crate::types::{ActionResult, CreditSnapshot, StatusResult};
use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// 批次号：同一批的逐账号记录挂在同一个 id 下，事后能整批捞出来。
fn batch_id() -> String {
    format!("batch-{}", Local::now().format("%Y%m%d-%H%M%S"))
}

/// 写一条逐账号记录。**成功也要写**——成功那行要能看出耗时和奖励，
/// 否则「全绿」和「全都没跑」在日志里长得一样。
fn record(
    subject: &str,
    step: &str,
    outcome: Outcome,
    reason: &str,
    evidence: &str,
    started: std::time::Instant,
    batch: Option<&str>,
) {
    let mut item = Item::new("签到", subject, outcome)
        .step(step)
        .reason(reason)
        .evidence(evidence)
        .ms(started.elapsed().as_millis() as u64);
    if let Some(batch) = batch {
        item = item.batch(batch);
    }
    log::item(item);
}

/// 批次收尾：`成功 + 跳过 + 失败` 必须等于 `总计`，所以跳过用减法算——
/// 三个数字对不上就说明有账号既没成功也没被计进失败，那是记录漏了。
fn finish_batch(
    action: &str,
    batch_id: &str,
    total: usize,
    round: &RoundResult,
    failures: &[String],
    trigger: &str,
    started: std::time::Instant,
) {
    let skipped = total.saturating_sub(round.success + round.failed);
    let detail = if failures.is_empty() {
        String::new()
    } else {
        format!(" | 失败明细: {}", failures.join("；"))
    };
    log::action(
        action,
        trigger,
        round.failed == 0,
        started.elapsed().as_millis() as u64,
        format!(
            "批次 {batch_id} 结束 | 总计={total} | 成功={} | 跳过={skipped} | 失败={}{detail}",
            round.success, round.failed
        ),
    );
}

/// 写盘失败的兜底：不中断签到流程，但必须留痕（红线：不裸吞写盘错误）。
/// 日志只带平台前缀，不带完整 iid——界面与日志不显示 uid。
fn log_write_failure(what: &str, scope: &str, e: AppError) {
    crate::app::log::log(
        crate::app::log::Level::Warn,
        "checkin",
        format!("{what}（{scope}）：{}", e.message),
    );
}

/// 日志里只留平台前缀，不出现完整 iid。
fn platform_scope(iid: &str) -> String {
    iid.split(':').next().unwrap_or(iid).to_string()
}

/// 「领取请求可能已发出、但结果不确定」的错误类别。
///
/// 这些失败后保留 `reserved`，让下一轮只回查、不重复领取；其余（凭证失效 / 活动
/// 关闭 / 配置无效 / 限流等）确定没有提交出去，直接清除标记，不挡用户重试。
/// 限流（429）不算：平台明确拒了，等于没提交。
fn uncertain_submit(kind: ErrorKind) -> bool {
    // 只有「请求可能已发出、但没拿到响应」才需要保留 reserved 等下一轮回查：
    // 典型是超时 / 连接中断。平台明确回了 4xx/5xx（Provider）说明这一发已经落地，
    // 失败就是失败，清掉标记让用户还能手动重试；否则一次 500 会把账号锁一整天。
    matches!(kind, ErrorKind::Io | ErrorKind::Sqlite)
}

/// 定时轮询的平台节流判定：距上次成功查询不足平台最小间隔 → 本轮跳过。
///
/// - `min_minutes = 0`：平台不设限，跟随全局任务间隔；
/// - `last_ok_at` 为空：从没查成功过，本轮必须查（不能让新账号一直等着）。
fn throttled(min_minutes: u64, last_ok_at: Option<i64>, now_ms: i64) -> bool {
    if min_minutes == 0 {
        return false;
    }
    match last_ok_at {
        Some(at) => now_ms.saturating_sub(at) < (min_minutes as i64) * 60_000,
        None => false,
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 一轮轮询的统计。界面只拿它拼一句提示文案。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoundResult {
    pub new_day: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub scanned: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub due: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub success: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub already: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub failed: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub waiting: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub skipped: usize,
    /// 本轮结束时刻（RFC3339），落 kv 供界面显示「上一轮」
    pub at: String,
    pub message: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 跨账号的一条领取记录（仪表盘「最近领取」用）。
#[derive(Debug, Clone, Serialize)]
pub struct RecentClaim {
    pub iid: String,
    /// 展示名（已脱敏/别名）
    pub name: String,
    pub source: String,
    pub amount: Option<f64>,
    pub description: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub claimed_at: i64,
}

/// 把「(date, iid, 当日首笔余额)」明细聚合成按天的收支。
///
/// 首笔余额是**当天起点**：某天的变动 = 下一天首笔（或今天的最新余额）− 当天首笔。
/// 因此差值归属于「发生变动的那一天」，而不是下一天；否则整张图会整体错一天。
fn aggregate_flow(
    rows: Vec<(String, String, f64)>,
    latest: &std::collections::HashMap<String, f64>,
    today: &str,
) -> Vec<CreditFlowPoint> {
    let mut per_account: std::collections::BTreeMap<String, Vec<(String, f64)>> =
        Default::default();
    for (date, iid, balance) in rows {
        per_account.entry(iid).or_default().push((date, balance));
    }

    let mut by_date: std::collections::BTreeMap<String, CreditFlowPoint> = Default::default();
    for (iid, mut series) in per_account {
        series.sort_by(|a, b| a.0.cmp(&b.0));
        for (index, (date, balance)) in series.iter().enumerate() {
            // 历史日：下一天的首笔就是当天结束时的余额；
            // 今天：最新一次查询才是「现在」，没有就退回首笔。
            let end = series.get(index + 1).map(|pair| pair.1).or_else(|| {
                if date == today {
                    latest.get(&iid).copied()
                } else {
                    None
                }
            });
            let point = by_date
                .entry(date.clone())
                .or_insert_with(|| CreditFlowPoint {
                    date: date.clone(),
                    earned: 0.0,
                    spent: 0.0,
                    net: 0.0,
                    balance: 0.0,
                });
            point.balance += end.unwrap_or(*balance);
            let Some(end) = end else {
                continue;
            };
            let delta = end - *balance;
            // 浮点噪声（1e-12 级）不算变动
            if delta.abs() < 0.005 {
                continue;
            }
            if delta > 0.0 {
                point.earned += delta;
            } else {
                point.spent += -delta;
            }
        }
    }

    by_date
        .into_values()
        .map(|mut point| {
            point.net = point.earned - point.spent;
            point
        })
        .collect()
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 一天的积分收支：由「按账号的日余额增量」汇总而来。
///
/// 正增量记 `earned`（签到/活动/旅行到账），负增量记 `spent`（平台侧消耗）。
/// 平台不提供消费流水，这是**唯一能观测到消耗的途径**，所以只做汇总、不做明细归因。
#[derive(Debug, Clone, Serialize)]
pub struct CreditFlowPoint {
    pub date: String,
    pub earned: f64,
    pub spent: f64,
    pub net: f64,
    /// 当日末全部账号余额合计
    pub balance: f64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 一个软件的积分收支。口径与总览完全一致，区别只是**先分组再汇总**：
/// 跨软件直接相加会把「A 号到账 100、B 号消耗 100」抹成没变化。
#[derive(Debug, Clone, Serialize)]
pub struct CreditPlatformFlow {
    /// 展示键：`workbuddy` / `workbuddy_intl` / `zcode` …
    pub platform: String,
    /// 账号表里的平台名（`platform_label`）
    pub label: String,
    /// 参与统计的账号数
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub accounts: usize,
    /// 按天明细，日期轴与总览一致
    pub points: Vec<CreditFlowPoint>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 积分收支的两层口径：全部账号合计 + 按软件拆分。
#[derive(Debug, Clone, Serialize)]
pub struct CreditFlowReport {
    /// 全部账号合计（按天，升序）
    pub total: Vec<CreditFlowPoint>,
    /// 每个软件一条，按区间消耗降序
    pub platforms: Vec<CreditPlatformFlow>,
}

/// iid → (软件展示键, 中文名)。
///
/// WorkBuddy 国内版与国际版在账号表里共用 `platform = "workbuddy"`，积分却各算各的，
/// 所以靠 iid 的 `@intl` 后缀拆成两条；账号表里查不到（已删除）就按平台前缀兜底。
fn platform_of(iid: &str, view: Option<&AccountView>) -> (String, String) {
    let platform = view
        .map(|view| view.platform.as_str())
        .unwrap_or_else(|| iid.split(':').next().unwrap_or(iid));
    let key = if platform == "workbuddy" && crate::app::accounts::is_intl_account(iid) {
        "workbuddy_intl"
    } else {
        platform
    };
    let label = view
        .map(|view| view.platform_label.clone())
        .unwrap_or_else(|| key.to_string());
    (key.to_string(), label)
}

#[derive(Default)]
struct PlatformGroup {
    label: String,
    rows: Vec<(String, String, f64)>,
    accounts: std::collections::HashSet<String>,
}

/// 按软件分组后各自沿用总览的算法。排序按区间消耗降序——「钱花在哪」是第一眼要回答的问题。
fn aggregate_platforms(
    rows: Vec<(String, String, f64)>,
    views: &std::collections::HashMap<String, AccountView>,
    latest: &std::collections::HashMap<String, f64>,
    today: &str,
) -> Vec<CreditPlatformFlow> {
    let mut groups: std::collections::HashMap<String, PlatformGroup> = Default::default();
    for (date, iid, balance) in rows {
        let (key, label) = platform_of(&iid, views.get(&iid));
        let group = groups.entry(key.clone()).or_default();
        // 已删除账号排在前面时会先占住兜底名（就是平台键），后续有名字的账号要能改回来
        if group.label.is_empty() || group.label == key {
            group.label = label;
        }
        group.accounts.insert(iid.clone());
        group.rows.push((date, iid, balance));
    }

    let mut out: Vec<CreditPlatformFlow> = groups
        .into_iter()
        .map(|(platform, group)| CreditPlatformFlow {
            platform,
            label: group.label,
            accounts: group.accounts.len(),
            points: aggregate_flow(group.rows, latest, today),
        })
        .collect();
    out.sort_by(|a, b| {
        spent_of(b)
            .total_cmp(&spent_of(a))
            .then_with(|| a.platform.cmp(&b.platform))
    });
    out
}

fn spent_of(flow: &CreditPlatformFlow) -> f64 {
    flow.points.iter().map(|point| point.spent).sum()
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 积分趋势的一天。`date` 是本地自然日 `YYYY-MM-DD`。
#[derive(Debug, Clone, Serialize)]
pub struct TrendPoint {
    pub date: String,
    pub balance: f64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 详情面板要的数据。**全部来自本地库**，点开一行才查，不打接口。
#[derive(Debug, Clone, Serialize)]
pub struct AccountDetail {
    pub balance: Option<f64>,
    /// 契约 6：当天最新看到的余额 − 当天首笔基线
    pub today_delta: Option<f64>,
    pub lots: Vec<crate::types::CreditLot>,
    pub claims: Vec<store::CreditClaim>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub next_at: Option<i64>,
    pub checked_in: bool,
    /// 今天真正签到的时刻（不是刷新时刻）
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub checked_at: Option<i64>,
    /// 额度窗口：名称 → 已用百分比
    pub quota: std::collections::BTreeMap<String, f64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub quota_at: Option<i64>,
}

#[derive(Clone)]
pub struct Service {
    db: Arc<Db>,
    http: Arc<dyn Http>,
    clock: Arc<dyn Clock>,
}

impl Service {
    pub fn new(db: Arc<Db>, http: Arc<dyn Http>, clock: Arc<dyn Clock>) -> Self {
        Self { db, http, clock }
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    /// 取账号视图与凭据包。凭据只在这一次调用里存在，不往外传。
    async fn load(&self, iid: &str) -> Result<(AccountView, accounts::credential::Credential)> {
        accounts::load_credential(self.db.clone(), iid).await
    }

    fn provider_of(iid: &str) -> Result<Box<dyn providers::Provider>> {
        providers::for_iid(iid)
            .ok_or_else(|| AppError::new(ErrorKind::Unsupported, "这个平台的账号暂不支持该操作"))
    }

    /// 查签到状态。免签到的平台只查积分，不去「看」签到状态。
    pub async fn status(&self, iid: &str) -> Result<StatusResult> {
        let (view, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        let cooldown = provider.cooldown_hours();
        let requires = provider.requires_checkin();
        let (result, quota) = if requires {
            (provider.status(&cred, self.http.as_ref()).await?, None)
        } else {
            let snapshot = provider.credits(&cred, self.http.as_ref()).await?;
            // StatusResult 没有 quota 字段，额度得单独带下去，否则这一发会把刚查到的
            // 窗口百分比丢掉，列表只能显示上一次的旧值。
            let quota = Self::quota_map(&snapshot);
            (
                StatusResult {
                    checked_in: None,
                    summary: snapshot.summary,
                    balance: snapshot.balance,
                    lots: snapshot.lots,
                    next_at: None,
                },
                quota,
            )
        };
        self.persist(iid, &result, quota).await;
        // 平台的签到结论要落到本地：本工具没签过（用户在官方客户端签的）时，
        // 不补标记列表会一直停在「待查询」，每轮轮询还会再问一次接口；
        // 平台明确说没签也要记下来，否则早上打开每个账号都只显示「待查询」。
        // 只认接口明确给的 true/false——None 是「没查到」，别猜。
        match result.checked_in {
            Some(true) => {
                if let Err(e) = self.mark_today(iid).await {
                    log_write_failure("写入去重标记失败", &platform_scope(iid), e);
                }
            }
            Some(false) => {
                if cooldown > 0 {
                    let db = self.db.clone();
                    let clock = self.clock.clone();
                    let key = iid.to_string();
                    match blocking(move || {
                        SqliteCheckin::new(&db).clear_today_mark(&key, clock.as_ref())
                    })
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) | Err(e) => {
                            log_write_failure("校正签到标记失败", &platform_scope(iid), e)
                        }
                    }
                }
                self.note_unchecked(iid).await;
            }
            None => {}
        }
        if requires && result.next_at.is_none() {
            self.fallback_next_at(iid, cooldown).await;
        }
        let _ = view;
        Ok(result)
    }

    /// 额度窗口转成有序表：CreditView 按 BTreeMap 存，界面显示顺序才稳定。
    fn quota_map(snapshot: &CreditSnapshot) -> Option<std::collections::BTreeMap<String, f64>> {
        snapshot.quota.as_ref().map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), *value))
                .collect()
        })
    }

    /// 只查积分（高频刷新走这条）。
    pub async fn credits(&self, iid: &str) -> Result<CreditSnapshot> {
        let (_, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        let snapshot = provider.credits(&cred, self.http.as_ref()).await?;
        if let Some(balance) = snapshot.balance {
            self.note_balance(iid, balance).await;
        }
        let quota = Self::quota_map(&snapshot);
        self.note_view(iid, quota, snapshot.lots.clone(), snapshot.balance)
            .await;
        Ok(snapshot)
    }

    /// 批量刷积分：并发 3（与状态查询同口径），单账号失败不拖垮整批。
    /// 失败账号在 `error` 里带原因，界面只认这个字段，不靠数值是否为空反推。
    ///
    /// 失败必须逐条落日志：界面只会说「N 个查询失败」，不写后端就查不出
    /// 是哪几个账号、卡在哪一步、原文是什么。
    pub async fn credits_batch(
        &self,
        iids: &[String],
        trigger: &str,
    ) -> Vec<(String, CreditSnapshot)> {
        let total = iids.len();
        let batch_id = batch_id();
        let started = std::time::Instant::now();
        // 失败行要显示账号名而不是 uid：先取一次名字表，查不到就退回平台前缀
        let names = self.account_names().await;
        if total > 0 {
            progress::start("checkin_credits", &batch_id, total);
        }

        let semaphore = Arc::new(Semaphore::new(3));
        let mut tasks = tokio::task::JoinSet::new();
        for iid in iids {
            let service = self.clone();
            let permit = semaphore.clone();
            let iid = iid.clone();
            let batch = batch_id.clone();
            let name = names.get(iid.as_str()).cloned();
            tasks.spawn(async move {
                let _permit = permit.acquire_owned().await.ok();
                let account_started = std::time::Instant::now();
                let snapshot = match service.refresh_snapshot(&iid).await {
                    Ok(snapshot) => {
                        service.health_ok(&iid).await;
                        snapshot
                    }
                    Err(e) => {
                        service.health_fail(&iid, e.kind).await;
                        let subject = name.unwrap_or_else(|| platform_scope(&iid));
                        log::item(
                            Item::new("刷新积分", &subject, Outcome::of_error(e.kind))
                                .step("查询积分")
                                .evidence(e.evidence())
                                .batch(&batch)
                                .ms(account_started.elapsed().as_millis() as u64),
                        );
                        CreditSnapshot {
                            summary: e.message.clone(),
                            error: Some(e.message),
                            balance: None,
                            lots: None,
                            quota: None,
                        }
                    }
                };
                (iid, snapshot)
            });
        }
        let mut out = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok(pair) = joined {
                let tag = if pair.1.error.is_none() { "ok" } else { "fail" };
                progress::tick("checkin_credits", &batch_id, tag);
                out.push(pair);
            } else {
                // 任务被取消/panic：这一项不会进 out，但用户看到的计数必须往前走
                progress::tick("checkin_credits", &batch_id, "fail");
            }
        }
        if total > 0 {
            progress::end("checkin_credits", &batch_id);
        }

        // 失败明细：界面上的「N 个查询失败」必须能和日志对上账
        let failures: Vec<String> = out
            .iter()
            .filter_map(|(iid, snapshot)| {
                snapshot.error.as_ref().map(|error| {
                    let subject = names
                        .get(iid.as_str())
                        .cloned()
                        .unwrap_or_else(|| platform_scope(iid));
                    format!("{subject} → {error}")
                })
            })
            .collect();
        let failed = failures.len();
        let detail = if failures.is_empty() {
            String::new()
        } else {
            format!(" | 失败明细: {}", failures.join("；"))
        };
        // 批次收尾：界面上的「N 个查询失败」必须能和日志对上账。
        // 定时触发且全绿时不记：默认 5 分钟一轮，全绿时每条都记等于刷屏，
        // 由调度器按「有没有失败」决定是否落记录。
        if trigger != "schedule" || failed > 0 {
            log::action(
                "checkin_credits",
                trigger,
                failed == 0,
                started.elapsed().as_millis() as u64,
                format!(
                    "批次 {batch_id} 结束 | 总计={total} | 成功={} | 失败={failed}{detail}",
                    total.saturating_sub(failed)
                ),
            );
        }
        out
    }

    /// 托盘「立即签到」用：所有启用、有凭据、需要签到的账号。
    pub async fn enabled_checkin_iids(&self) -> Vec<String> {
        self.accounts()
            .await
            .map(|list| {
                list.into_iter()
                    .filter(|a| a.requires_checkin && a.has_secret && a.enabled)
                    .map(|a| a.iid)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 批量签到的节奏：并发上限 + 账号间隔。来自应用配置（data/config.toml 或设置界面）。
    async fn checkin_limits(&self) -> (usize, u64) {
        let config = crate::app::config::get();
        (
            config.checkin.max_concurrent.clamp(1, 6),
            config.checkin.gap_ms.min(60_000),
        )
    }

    /// 测试用：读取批量节奏配置。
    #[doc(hidden)]
    pub async fn batch_limits_for_test(&self) -> (usize, u64) {
        self.checkin_limits().await
    }

    async fn health_ok(&self, iid: &str) {
        let db = self.db.clone();
        let key = iid.to_string();
        let now = self.clock.now().timestamp_millis();
        let _ = blocking(move || accounts::repo::note_health_ok(&db, &key, now)).await;
    }

    async fn health_fail(&self, iid: &str, kind: ErrorKind) {
        // 断网 / 限流不算账号坏：不落健康标记，否则账号中心会一路红到下次成功。
        // 这类失败仍会进动作日志（记为「待重试」），可查但不误导。
        if kind.is_transient() {
            return;
        }
        let db = self.db.clone();
        let key = iid.to_string();
        let now = self.clock.now().timestamp_millis();
        let kind = kind.as_str().to_string();
        let _ = blocking(move || accounts::repo::note_health_fail(&db, &key, &kind, now)).await;
    }

    /// iid → 账号名。只为日志服务，读不到就退化成平台前缀。
    async fn account_names(&self) -> std::collections::HashMap<String, String> {
        self.accounts()
            .await
            .map(|list| {
                list.into_iter()
                    .map(|account| {
                        let name = account.display_name();
                        (account.iid, name)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 「刷新积分」实际打的那一发：只查积分接口，不再顺带查签到状态。
    ///
    /// 旧实现为了让「用户在官方客户端签过」的账号能显示已签，每次积分刷新都先打
    /// 一次签到状态接口——5 分钟一轮、十几个账号，等于把状态端点也变成了脚本样本。
    /// 现在签到状态只由本地 `daily_marks` 回答；官方客户端签过的账号，下一次签到
    /// 提交会由平台返回「已签」，服务层再落本地标记。
    async fn refresh_snapshot(&self, iid: &str) -> Result<CreditSnapshot> {
        self.credits(iid).await
    }

    /// 定时积分刷新：全部有凭据的账号刷一遍余额。返回 (总数, 失败数)。
    ///
    /// 平台可以声明自己的最小刷新间隔（`Provider::credits_min_interval_minutes`）：
    /// 距离上次成功查询不够久的账号，这一轮直接跳过——豆包的活动面板接口
    /// 按 5 分钟一问是 288 次/天，节奏完全不像人。手动刷新不走这里，不受限制。
    pub async fn credits_round(&self) -> (usize, usize) {
        let accounts = match self.accounts().await {
            Ok(list) => list,
            Err(_) => return (0, 0),
        };
        let now = self.clock.now().timestamp_millis();
        let mut iids = Vec::new();
        for account in accounts.into_iter().filter(|a| a.has_secret && a.enabled) {
            let min = Self::provider_of(&account.iid)
                .map(|provider| provider.credits_min_interval_minutes())
                .unwrap_or(0);
            if throttled(min, account.last_ok_at, now) {
                continue;
            }
            iids.push(account.iid);
        }
        let total = iids.len();
        if total == 0 {
            return (0, 0);
        }
        let out = self.credits_batch(&iids, "schedule").await;
        // 只认显式错误标记：成功但没拿到数值的账号不算失败
        let failed = out.iter().filter(|(_, s)| s.error.is_some()).count();
        (total, failed)
    }

    /// 详情面板：只读本地库（余额、今日增减、批次、下次可签、额度），一次都不打接口。
    pub async fn detail(&self, iid: &str) -> AccountDetail {
        let checked_in = self.marked_today(iid).await;
        let today = self.clock.today();
        let stamp = today.format("%Y-%m-%d").to_string();
        let db = self.db.clone();
        let key = iid.to_string();
        let (balance, delta, lots, next_at, checked_at, quota, quota_at) = blocking(move || {
            let store = SqliteCheckin::new(&db);
            let view = store.view_of(&key);
            // 详情显示今天的最新余额；没有实时缓存才退回当日首笔基线
            let balance = view
                .as_ref()
                .and_then(|item| {
                    let at = chrono::Local.timestamp_millis_opt(item.at).single()?;
                    if at.date_naive() == today {
                        item.balance
                    } else {
                        None
                    }
                })
                .or_else(|| store.balance_on(&key, &stamp));
            // 实时批次比台账更接近「现在还剩什么」；没有就退回台账
            let mut lots = view
                .as_ref()
                .map(|item| store::lots_for_display(&key, item.lots.clone()))
                .unwrap_or_default();
            if lots.is_empty() && !accounts::is_intl_account(&key) {
                // 国际版不退回台账：它的批次到期不可信，旧日期也不该翻出来
                lots = store::lots_of(&db, &key, 10);
            }
            (
                balance,
                store::today_delta(&db, &key, today),
                lots,
                store.next_at(&key).map(|at| at.timestamp_millis()),
                store.last_checkin_at(&key).map(|at| at.timestamp_millis()),
                view.as_ref()
                    .map(|item| item.quota.clone())
                    .unwrap_or_default(),
                view.map(|item| item.at),
            )
        })
        .await
        .unwrap_or((None, None, Vec::new(), None, None, Default::default(), None));
        let db = self.db.clone();
        let key = iid.to_string();
        let claims = match blocking(move || SqliteCheckin::new(&db).claims(&key)).await {
            Ok(Ok(claims)) => claims,
            Ok(Err(e)) | Err(e) => {
                log::fail(
                    "checkin",
                    "credit_claim_read",
                    "读取积分领取记录失败",
                    e.evidence(),
                );
                Vec::new()
            }
        };
        AccountDetail {
            claims,
            balance,
            today_delta: delta,
            lots,
            next_at,
            checked_in,
            checked_at,
            quota,
            quota_at,
        }
    }

    /// 积分收支：一次取齐「全部账号合计」与「按软件拆分」两层口径。
    ///
    /// 平台只给余额，不给消费明细；把每个账号相邻两天的余额差相加，
    /// 正数是到账、负数是消耗。缺数据的日子不补零，避免把「没记录」画成「没变化」。
    /// 窗口由界面给定（`window.rs`），不设天数上限。
    pub async fn credit_flow(&self, window: DayWindow) -> CreditFlowReport {
        let today = self.clock.today();
        // 收支是「相邻两天的余额差」：窗口起点再往前取一天当基线，否则窗口第一天
        // 只有一条快照、永远算不出变动。基线日只参与计算，不返回给界面。
        let since = (window.from - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let today_stamp = today.format("%Y-%m-%d").to_string();
        let db = self.db.clone();
        let rows = match blocking(move || SqliteCheckin::new(&db).balance_rows(&since)).await {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) | Err(e) => {
                log::fail("checkin", "credit_flow", "读取积分收支失败", e.evidence());
                return CreditFlowReport {
                    total: Vec::new(),
                    platforms: Vec::new(),
                };
            }
        };
        // 今天的最新余额在展示缓存里（每日快照只保留首笔）；历史日仍用快照。
        let db = self.db.clone();
        let latest = match blocking(move || SqliteCheckin::new(&db).latest_balances(today)).await {
            Ok(Ok(map)) => map,
            Ok(Err(e)) | Err(e) => {
                log::fail("checkin", "credit_flow", "读取最新余额失败", e.evidence());
                std::collections::HashMap::new()
            }
        };
        // 账号表只用来拿平台展示名：账目本身只认 credit_snapshots 里的 iid
        let views: std::collections::HashMap<String, AccountView> = self
            .accounts()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|view| (view.iid.clone(), view))
            .collect();
        let mut platforms = aggregate_platforms(rows.clone(), &views, &latest, &today_stamp);
        let mut total = aggregate_flow(rows, &latest, &today_stamp);
        // 裁掉基线日、以及窗口终点之后的日子：界面只看请求的那个区间
        let (from, to) = (window.from_iso(), window.to_iso());
        total.retain(|point| point.date >= from && point.date <= to);
        for platform in &mut platforms {
            platform
                .points
                .retain(|point| point.date >= from && point.date <= to);
        }
        CreditFlowReport { total, platforms }
    }

    /// 跨账号最近领取：仪表盘用，最多 100 条。
    pub async fn recent_claims(&self, limit: usize) -> Vec<RecentClaim> {
        let names = self.account_names().await;
        let db = self.db.clone();
        match blocking(move || SqliteCheckin::new(&db).recent_claims(limit)).await {
            Ok(Ok(rows)) => rows
                .into_iter()
                .map(|(iid, claim)| RecentClaim {
                    name: names
                        .get(&iid)
                        .cloned()
                        .unwrap_or_else(|| platform_scope(&iid)),
                    iid,
                    source: claim.source,
                    amount: claim.amount,
                    description: claim.description,
                    claimed_at: claim.claimed_at,
                })
                .collect(),
            Ok(Err(e)) | Err(e) => {
                log::fail(
                    "checkin",
                    "credit_claim_read",
                    "读取最近领取失败",
                    e.evidence(),
                );
                Vec::new()
            }
        }
    }

    /// 积分趋势：读每日余额快照，窗口由界面给定。`iid` 为空时汇总全部账号。
    pub async fn credit_trend(&self, iid: Option<String>, window: DayWindow) -> Vec<TrendPoint> {
        let since = window.from_iso();
        let to = window.to_iso();
        let db = self.db.clone();
        match blocking(move || SqliteCheckin::new(&db).snapshot_series(iid.as_deref(), &since))
            .await
        {
            Ok(Ok(rows)) => rows
                .into_iter()
                .filter(|(date, _)| date.as_str() <= to.as_str())
                .map(|(date, balance)| TrendPoint { date, balance })
                .collect(),
            Ok(Err(e)) | Err(e) => {
                log::fail("checkin", "credit_trend", "读取积分趋势失败", e.evidence());
                Vec::new()
            }
        }
    }

    /// 单账号签到。**幂等在这**：当日已签直接返回，绝不调接口。
    pub async fn checkin(&self, iid: &str) -> Result<ActionResult> {
        self.checkin_traced(iid, None).await.1
    }
    /// 带执行记录的单账号签到，返回 (账号名, 结果)。`batch` 有值时挂在批次下。
    ///
    /// 记录写在这一层而不是 `batch` 里：只有这里知道失败卡在哪一步
    /// （读凭据 / 签到前检查 / 提交签到 / 落库），`batch` 手上只有一个 Result，
    /// 写不出「环节」，最后只能落一句「部分账号失败」——那正是要避免的废话。
    async fn checkin_traced(
        &self,
        iid: &str,
        batch: Option<&str>,
    ) -> (String, Result<ActionResult>) {
        let started = std::time::Instant::now();
        let mut subject = iid.to_string();

        let (view, cred) = match self.load(iid).await {
            Ok(pair) => {
                subject = pair.0.display_name();
                pair
            }
            Err(e) => {
                record(
                    &subject,
                    "读取凭据",
                    Outcome::Fail,
                    &e.message,
                    &format!("账号={iid}"),
                    started,
                    batch,
                );
                return (subject, Err(e));
            }
        };
        let provider = match Self::provider_of(iid) {
            Ok(provider) => provider,
            Err(e) => {
                record(
                    &subject,
                    "签到前检查",
                    Outcome::Fail,
                    &e.message,
                    &format!("账号={iid}"),
                    started,
                    batch,
                );
                return (subject, Err(e));
            }
        };

        if !provider.requires_checkin() {
            let result = ActionResult {
                ok: false,
                message: format!("{} 无需签到", view.platform_label),
                granted: None,
                validity_days: None,
                already: false,
            };
            record(
                &subject,
                "签到前检查",
                Outcome::Skip,
                &result.message,
                &format!("平台={}", view.platform_label),
                started,
                batch,
            );
            return (subject, Ok(result));
        }
        // 本机验证优先：所有平台一律先看 daily_marks，命中就当天不再碰任何接口。
        // 若只看 cooldown_hours()==0，Qoder 这类 24 小时窗口的平台
        // 会绕过本地表重复提交——那正是账号被风控的入口之一。
        if self.marked_today(iid).await {
            let result = ActionResult {
                ok: true,
                message: "今日已签到（本工具的记录）".into(),
                granted: None,
                validity_days: None,
                already: true,
            };
            record(
                &subject,
                "签到前检查",
                Outcome::Skip,
                &result.message,
                "依据=本地去重表 daily_marks 命中",
                started,
                batch,
            );
            return (subject, Ok(result));
        }

        // 今天已有提交但结果未确认（reserved）：只回查、不重复领取。这是跨轮次幂等
        // 的第二道防线——上一轮「领取已发出、响应丢了」时，本轮不能再发第二次。
        if self.reserved_today(iid).await {
            if let Ok(status) = provider.status(&cred, self.http.as_ref()).await {
                if status.checked_in == Some(true) {
                    let _ = self.mark_today(iid).await;
                    self.settle_today(iid).await;
                    let result = ActionResult {
                        ok: true,
                        message: "今日签到已由平台确认（提交后回查）".into(),
                        granted: None,
                        validity_days: None,
                        already: true,
                    };
                    record(
                        &subject,
                        "签到前检查",
                        Outcome::Skip,
                        &result.message,
                        "依据=checkin_attempts.reserved 回查命中",
                        started,
                        batch,
                    );
                    return (subject, Ok(result));
                }
            }
            let result = ActionResult {
                ok: true,
                message: "今日签到已提交、结果待确认，本轮只回查不重复领取".into(),
                granted: None,
                validity_days: None,
                already: true,
            };
            record(
                &subject,
                "签到前检查",
                Outcome::Skip,
                &result.message,
                "依据=checkin_attempts.reserved 未确认",
                started,
                batch,
            );
            return (subject, Ok(result));
        }

        // 签到前先查一次积分：把当天第一笔余额落库，作为「今天用了/到账多少」的基线。
        // 必须发生在真正提交之前，否则签到后的余额会被当成起点，今天的到账就永远算不出来。
        // 查询失败不阻断签到——拿不到基线也比错过当天签到强，失败会单独落一条日志。
        let mut before_balance = None;
        match provider.credits(&cred, self.http.as_ref()).await {
            Ok(snapshot) => {
                before_balance = snapshot.balance;
                if let Some(balance) = snapshot.balance {
                    self.note_balance(iid, balance).await;
                }
                let quota = Self::quota_map(&snapshot);
                self.note_view(iid, quota, snapshot.lots.clone(), snapshot.balance)
                    .await;
            }
            Err(e) => {
                record(
                    &subject,
                    "签到前查积分",
                    Outcome::of_error(e.kind),
                    &e.message,
                    &format!("平台={} 账号={iid} 基线未写入", view.platform_label),
                    started,
                    batch,
                );
            }
        }

        // 提交前先打 reserved 标记：这一步之后平台可能已处理、响应却丢了，
        // 标记先落库，后续轮次据此只回查不重领。
        self.reserve_today(iid).await;

        // 只发一次真实签到：失败不自动重试，避免把「一次失败」放大成三次请求。
        // 请求是否到达平台无法本地判断，因此不确定的失败保留 reserved，
        // 由下一轮做一次状态回查；确定没提交出去的直接清除标记。
        let result = match provider.checkin(&cred, self.http.as_ref()).await {
            Ok(value) => {
                self.settle_today(iid).await;
                value
            }
            Err(e) => {
                if !uncertain_submit(e.kind) {
                    self.settle_today(iid).await;
                }
                self.health_fail(iid, e.kind).await;
                record(
                    &subject,
                    "提交签到",
                    Outcome::of_error(e.kind),
                    &e.message,
                    &format!(
                        "平台={} 账号={iid} 提交=1次 · {}",
                        view.platform_label,
                        e.evidence()
                    ),
                    started,
                    batch,
                );
                return (subject, Err(e));
            }
        };
        if result.ok {
            self.health_ok(iid).await;
        } else {
            self.health_fail(iid, ErrorKind::Provider).await;
        }

        // 只要平台给了确定性成功（含「今天已签」），当天就封闸：写 daily_marks。
        // 后续轮次在 checkin_traced 开头直接返回，不再调签到接口，直到第二天。
        if result.ok {
            if let Err(e) = self.mark_today(iid).await {
                record(
                    &subject,
                    "记录签到结果",
                    Outcome::Fail,
                    &e.message,
                    &e.evidence(),
                    started,
                    batch,
                );
            }
            // 下次可签时刻按本地规则推算，不再为了拿 next_at 额外查一次状态
            self.fallback_next_at(iid, provider.cooldown_hours()).await;
        }
        // 「本轮早领过」没有新发放：只写去重标记，不记领取台账
        if result.ok && !result.already {
            // 平台给了发放金额时，用「签到前余额 + 发放」本地推算最新余额，
            // 不用为了刷新数字再打一次积分接口；拿不到金额就等下一轮定时刷新。
            if let (Some(before), Some(granted)) = (before_balance, result.granted) {
                if granted > 0.0 {
                    self.note_view(iid, None, None, Some(before + granted))
                        .await;
                }
            }
            let claimed_at = self.clock.now().timestamp_millis();
            store::record_claim(
                self.db.clone(),
                iid,
                store::CreditClaim {
                    event_key: format!("checkin:{claimed_at}"),
                    source: "checkin".into(),
                    amount: result.granted,
                    description: "签到领取".into(),
                    claimed_at,
                },
            )
            .await;
            if let Some(granted) = result.granted.filter(|g| *g > 0.0) {
                self.record_grant(iid, granted, result.validity_days, provider.live_credit())
                    .await;
            }
        }

        // 结果归类：跳过 ≠ 失败。跳过要写清依据，失败要写清证据
        let (outcome, evidence) = if result.already {
            (
                Outcome::Skip,
                format!("平台={} 接口=签到 返回=今日已签", view.platform_label),
            )
        } else if result.ok {
            let granted = match (result.granted, result.validity_days) {
                (Some(g), Some(days)) => format!("奖励=+{g} 有效期={days}天"),
                (Some(g), None) => format!("奖励=+{g}"),
                _ => "奖励=无新发放".to_string(),
            };
            (
                Outcome::Ok,
                format!("平台={} {granted}", view.platform_label),
            )
        } else {
            (
                Outcome::Fail,
                format!("平台={} 账号={iid}", view.platform_label),
            )
        };
        record(
            &subject,
            "提交签到",
            outcome,
            &result.message,
            &evidence,
            started,
            batch,
        );
        (subject, Ok(result))
    }

    /// 批量签到：并发 3（平台风控上限）。
    ///
    /// 记录分三层：批次开始 → 每个账号一条（在 `checkin_traced` 里）→ 批次结束。
    /// 结束行的 `成功 + 跳过 + 失败` 恒等于 `总计`（跳过用减法算），对不上就是有漏。
    pub async fn batch(&self, iids: &[String], trigger: &str) -> RoundResult {
        // 停用账号不参与自动批次：前端可能已经过滤，但后端必须再守一次
        let accounts = self.accounts().await.unwrap_or_default();
        let enabled: std::collections::HashSet<&str> = accounts
            .iter()
            .filter(|a| a.enabled)
            .map(|a| a.iid.as_str())
            .collect();
        let wanted: Vec<String> = iids
            .iter()
            .filter(|iid| enabled.contains(iid.as_str()))
            .cloned()
            .collect();
        let total = wanted.len();
        let batch_id = batch_id();
        let started = std::time::Instant::now();
        let (max_concurrent, gap_ms) = self.checkin_limits().await;
        log::item(log::Item::marker("批次", &batch_id).note(format!(
            "开始 | 动作=批量签到 | 账号数={total} | 并发={max_concurrent} | 间隔={gap_ms}ms"
        )));
        // 界面按钮旁的「N / total」：总数只有这里知道，完成数前端按 tick 累加
        if total > 0 {
            progress::start("checkin_batch", &batch_id, total);
        }

        let mut round = RoundResult {
            scanned: total,
            ..Default::default()
        };
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let mut tasks = tokio::task::JoinSet::new();

        for (index, iid) in wanted.iter().enumerate() {
            let service = self.clone();
            let permit_sem = semaphore.clone();
            let batch = batch_id.clone();
            let iid = iid.clone();
            tasks.spawn(async move {
                let _permit = permit_sem.acquire_owned().await.ok();
                service.checkin_traced(&iid, Some(&batch)).await
            });
            // 账号间隔控制「开始时间」，并发上限仍由 Semaphore 管；两者独立可配
            if gap_ms > 0 && index + 1 < wanted.len() {
                tokio::time::sleep(std::time::Duration::from_millis(gap_ms)).await;
            }
        }

        let mut failures: Vec<String> = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            // 结果分类与进度标签一起算：分类写在 match 里，标签必须同源，
            // 否则「成功 3」和条上的 3 格可能对不上
            let outcome = match joined {
                Ok((subject, Ok(result))) => {
                    round.due += 1;
                    if result.ok && !result.already {
                        round.success += 1;
                        "ok"
                    } else if result.ok {
                        round.already += 1;
                        "skip"
                    } else {
                        round.failed += 1;
                        failures.push(format!("{subject} → {}", result.message));
                        "fail"
                    }
                }
                Ok((subject, Err(e))) => {
                    round.failed += 1;
                    failures.push(format!("{subject} → {}", e.message));
                    "fail"
                }
                Err(_) => {
                    round.failed += 1;
                    "fail"
                }
            };
            progress::tick("checkin_batch", &batch_id, outcome);
        }
        round.at = self.now_text();
        round.message = round.summary();
        finish_batch(
            "checkin_batch",
            &batch_id,
            total,
            &round,
            &failures,
            trigger,
            started,
        );
        if total > 0 {
            progress::end("checkin_batch", &batch_id);
        }
        round
    }

    /// 跑一轮完整流程。`new_day` 靠 `daily_marks` 里的日期判断。
    ///
    /// `trigger` 只用于执行记录（`user` 手动点 / `schedule` 定时），不影响业务。
    pub async fn run_round(&self, trigger: &str) -> RoundResult {
        let today = self.clock.today().format("%Y-%m-%d").to_string();
        let new_day = self.is_new_day(&today);
        let batch_id = batch_id();
        let started = std::time::Instant::now();
        let mut round = RoundResult {
            new_day,
            ..Default::default()
        };

        let accounts = match self.accounts().await {
            Ok(list) => list,
            Err(e) => {
                round.at = self.now_text();
                round.message = format!("读取账号失败：{e}");
                log::action(
                    "checkin_round",
                    trigger,
                    false,
                    started.elapsed().as_millis() as u64,
                    format!("批次 {batch_id} 未能开始 | 环节=读取账号 | 原因={e}"),
                );
                return round;
            }
        };
        round.scanned = accounts.len();
        let targets: Vec<AccountView> = accounts
            .into_iter()
            .filter(|a| a.requires_checkin && a.has_secret && a.enabled)
            .collect();

        // 第二步：只查本机记录，不再为每个账号调一次状态接口。
        // 定时轮询本身也要算进平台风控，本地 daily_marks 已经能回答「今天签没签」。
        // 上一轮「已提交但响应丢失」的账号（reserved）交给 checkin_traced 做一次回查，
        // 这里不提前查，避免同一轮里重复请求。
        let mut due: Vec<String> = Vec::new();
        for account in &targets {
            if self.marked_today(&account.iid).await {
                round.skipped += 1;
                continue;
            }
            due.push(account.iid.clone());
        }

        // 第三步：只对「今天还没签成功 + 现在确实可签」的账号调接口
        round.due = due.len();
        // 批次标记只在真的要调接口时写：同日轮询「无需签到」是正常心跳，
        // 每轮都留一条开始行只会把真正的签到记录淹掉。
        if !due.is_empty() {
            log::item(log::Item::marker("批次", &batch_id).note(format!(
                "开始 | 动作=签到轮询 | 账号总数={} | 参与签到={} | 新的一天={new_day}",
                round.scanned,
                targets.len()
            )));
            // 分母用 due.len()：今日已签的账号不进这一轮，不该出现在进度里
            progress::start("checkin_round", &batch_id, due.len());
        }
        let mut failures: Vec<String> = Vec::new();
        for iid in &due {
            let (subject, outcome) = self.checkin_traced(iid, Some(&batch_id)).await;
            let tag = match &outcome {
                Ok(r) if r.ok && !r.already => "ok",
                Ok(r) if r.ok => "skip",
                _ => "fail",
            };
            match outcome {
                Ok(result) if result.ok && !result.already => round.success += 1,
                Ok(result) if result.ok => round.already += 1,
                Ok(result) => {
                    round.failed += 1;
                    failures.push(format!("{subject} → {}", result.message));
                }
                Err(e) => {
                    round.failed += 1;
                    failures.push(format!("{subject} → {}", e.message));
                }
            }
            progress::tick("checkin_round", &batch_id, tag);
        }
        if !due.is_empty() {
            progress::end("checkin_round", &batch_id);
        }

        round.at = self.now_text();
        round.message = round.summary();
        // 定时触发的汇总由调度器统一记一条（带规则文案），这里不再重复记；
        // 手动/托盘触发仍由这里落一条执行记录。
        if trigger != "schedule" {
            finish_batch(
                "checkin_round",
                &batch_id,
                round.scanned,
                &round,
                &failures,
                trigger,
                started,
            );
        }
        self.save_daily_state(&today, &round);
        round
    }

    // ---------------------------------------------------------------- ZCode 活动领取
    // ZCode 活动领取：不是签到轮询的一部分（ZCode requires_checkin=false），
    // 是界面上单独的动作；领取会开浏览器过验证码。

    /// 平台奖励积分明细（WorkBuddy 个人中心「套餐与积分」那一页的口径）。
    ///
    /// 与积分余额的区别：余额只回答「现在有多少」，这里回答「哪几批、各剩多少、
    /// 什么时候到期」——正是网页上那张明细表的内容。
    pub async fn reward_credits(
        &self,
        iid: &str,
    ) -> Result<crate::app::providers::workbuddy::RewardCredits> {
        let (_, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        provider.credit_rewards(&cred, self.http.as_ref()).await
    }

    /// 可领取的活动套餐列表（preview 为主、marketing 补充）。
    pub async fn zcode_claimable(
        &self,
        iid: &str,
    ) -> Result<Vec<crate::app::providers::zclaim::ClaimPlan>> {
        let (_, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        provider.claim_plans(&cred, self.http.as_ref()).await
    }

    /// 领取一个套餐：开浏览器过验证码，拿到参数自动提交。3007 不自动重试。
    pub async fn zcode_claim(
        &self,
        iid: &str,
        plan_id: &str,
    ) -> Result<crate::app::providers::zclaim::ClaimOutcome> {
        let (view, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        let outcome = provider
            .claim_plan(&cred, self.http.as_ref(), plan_id)
            .await?;
        let name = if outcome.name.is_empty() {
            plan_id
        } else {
            &outcome.name
        };
        store::record_claim(
            self.db.clone(),
            iid,
            store::CreditClaim {
                event_key: format!(
                    "activity:{plan_id}:{}:{}",
                    outcome.effective_at, outcome.ends_at
                ),
                source: "activity".into(),
                amount: None,
                description: name.to_string(),
                claimed_at: self.clock.now().timestamp_millis(),
            },
        )
        .await;
        crate::app::log::log(
            crate::app::log::Level::Info,
            "checkin",
            format!("ZCode 活动领取成功：{} · {name}", view.display_name()),
        );
        Ok(outcome)
    }

    /// 自动领取一轮（开关 `zcode_auto_claim` 默认关）：
    /// 探测全部 ZCode 账号，找到可领的就领第一个，返回给人看的汇报行。
    pub async fn zcode_auto_claim_round(&self) -> Option<String> {
        let accounts = self.accounts().await.ok()?;
        for account in accounts
            .into_iter()
            .filter(|a| a.platform == "zcode" && a.has_secret)
        {
            let Ok(plans) = self.zcode_claimable(&account.iid).await else {
                continue;
            };
            let Some(plan) = plans.first() else { continue };
            let label = plan.name.clone();
            let line = match self.zcode_claim(&account.iid, &plan.plan_id).await {
                Ok(outcome) => format!(
                    "{}：已领取 {}",
                    account.display_name(),
                    if outcome.name.is_empty() {
                        label
                    } else {
                        outcome.name
                    }
                ),
                Err(e) => format!("{}：领取失败（{}）", account.display_name(), e.message),
            };
            return Some(line);
        }
        None
    }

    // ---------------------------------------------------------------- 内部

    async fn accounts(&self) -> Result<Vec<AccountView>> {
        let db = self.db.clone();
        blocking(move || accounts::repo::list(&db)).await?
    }

    async fn persist(
        &self,
        iid: &str,
        status: &StatusResult,
        quota: Option<std::collections::BTreeMap<String, f64>>,
    ) {
        if let Some(balance) = status.balance {
            self.note_balance(iid, balance).await;
        }
        self.note_next_at(iid, status.next_at).await;
        self.note_view(iid, quota, status.lots.clone(), status.balance)
            .await;
    }

    /// 把「平台此刻还剩什么」落成展示缓存。写失败只落日志：缓存丢了顶多显示旧值，
    /// 不能因此把一次成功的签到判成失败。
    async fn note_view(
        &self,
        iid: &str,
        quota: Option<std::collections::BTreeMap<String, f64>>,
        lots: Option<Vec<crate::types::CreditLot>>,
        balance: Option<f64>,
    ) {
        if quota.is_none() && lots.is_none() && balance.is_none() {
            return;
        }
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || {
            SqliteCheckin::new(&db).note_view(
                &iid,
                quota,
                lots,
                balance,
                clock.now().timestamp_millis(),
            )
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => log_write_failure("写入额度缓存失败", &scope, e),
        }
    }

    async fn note_balance(&self, iid: &str, balance: f64) {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || SqliteCheckin::new(&db).note_balance(&iid, balance, clock.as_ref()))
            .await
        {
            // 内层返回的是「有没有记一笔」的 bool，写盘失败才要看
            Ok(Ok(_)) => {}
            // 内层是写盘失败、外层是任务崩溃，都要留痕
            Ok(Err(e)) | Err(e) => log_write_failure("写入余额快照失败", &scope, e),
        }
    }

    /// 契约 22：接口没给 `next_at` 就把旧值删掉，否则重启后还会按老时刻「等待」。
    async fn note_next_at(&self, iid: &str, next: Option<DateTime<Local>>) {
        let db = self.db.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || {
            let store = SqliteCheckin::new(&db);
            match next {
                Some(at) => store.note_next_at(&iid, at),
                None => db.kv_delete(&format!("next_at:{iid}")),
            }
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => log_write_failure("写入下次可签时刻失败", &scope, e),
        }
    }

    /// 契约 4 的生产接线：活动制平台的下次可签时刻由接口下发，接口没给时
    /// 按本地记录推算（自然日次日 0 点 / 上次 + N 小时）。只推「今天已签成功」
    /// 的账号——没签的账号下次可签就是现在，没有「下次」可言。
    async fn fallback_next_at(&self, iid: &str, cooldown_hours: u32) {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || {
            let store = SqliteCheckin::new(&db);
            if store.marked_today(&iid, clock.as_ref()) {
                if let Some(last) = store.last_checkin_at(&iid) {
                    if let Some(next) = store.next_checkin_at(cooldown_hours, last, clock.as_ref())
                    {
                        return store.note_next_at(&iid, next);
                    }
                }
            }
            Ok(())
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => log_write_failure("写入推算的下次可签时刻失败", &scope, e),
        }
    }

    async fn marked_today(&self, iid: &str) -> bool {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        // 崩溃/读失败按「未标记」处理：多问一次接口，去重由服务端兜底
        blocking(move || SqliteCheckin::new(&db).marked_today(&iid, clock.as_ref()))
            .await
            .unwrap_or(false)
    }

    async fn mark_today(&self, iid: &str) -> Result<()> {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        blocking(move || SqliteCheckin::new(&db).mark_today(&iid, clock.as_ref())).await?
    }

    /// 今天是否已提交签到但结果未确认（`reserved`）。读失败按「未提交」处理。
    async fn reserved_today(&self, iid: &str) -> bool {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        blocking(move || SqliteCheckin::new(&db).reserved_today(&iid, clock.as_ref()))
            .await
            .unwrap_or(false)
    }

    /// 提交领取前打 reserved 标记；写失败只留痕，不中断签到（去重仍由服务端兜底）。
    async fn reserve_today(&self, iid: &str) {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || SqliteCheckin::new(&db).reserve_today(&iid, clock.as_ref())).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => log_write_failure("写入签到提交标记失败", &scope, e),
        }
    }

    /// 确认结果后清除 reserved。
    async fn settle_today(&self, iid: &str) {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || SqliteCheckin::new(&db).settle_today(&iid, clock.as_ref())).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => log_write_failure("清除签到提交标记失败", &scope, e),
        }
    }

    /// 「平台说今天没签」只影响展示，写失败不值得打断任何流程。
    async fn note_unchecked(&self, iid: &str) {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || SqliteCheckin::new(&db).note_unchecked(&iid, clock.as_ref())).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => log_write_failure("写入未签状态失败", &scope, e),
        }
    }

    async fn record_grant(
        &self,
        iid: &str,
        granted: f64,
        validity_days: Option<i64>,
        live_credit: bool,
    ) {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || {
            // 契约 5 的生产接线（add_if_needed）：live_credit=true 平台自己返回
            // 批次，台账不重复记——这条幂等判断收在契约实现里，不在调用方
            SqliteLedger::new(&db).add_if_needed(
                live_credit,
                &iid,
                granted,
                validity_days,
                clock.as_ref(),
            )
        })
        .await
        {
            Ok(Ok(_)) => {}
            // 台账只记本工具亲眼发放的批次，写失败就永久丢了，必须留痕
            Ok(Err(e)) | Err(e) => log_write_failure("写入积分台账失败", &scope, e),
        }
    }

    /// 当天还有没有更早的轮次记录：没有就是「新的一天」。
    fn is_new_day(&self, today: &str) -> bool {
        let previous = self
            .db
            .kv_get("daily:date")
            .ok()
            .flatten()
            .unwrap_or_default();
        let new_day = previous != today;
        if new_day {
            if let Err(e) = self.db.kv_set("daily:date", today) {
                log_write_failure("写入轮询日期失败", "daily", e);
            }
        }
        new_day
    }

    fn save_daily_state(&self, today: &str, round: &RoundResult) {
        if let Ok(text) = serde_json::to_string(round) {
            if let Err(e) = self.db.kv_set("daily:last", &text) {
                log_write_failure("写入上一轮统计失败", "daily", e);
            }
        }
        if let Err(e) = self.db.kv_set("daily:date", today) {
            log_write_failure("写入轮询日期失败", "daily", e);
        }
    }

    pub fn daily_state(&self) -> Option<RoundResult> {
        let raw = self.db.kv_get("daily:last").ok().flatten()?;
        serde_json::from_str(&raw).ok()
    }

    fn now_text(&self) -> String {
        self.clock
            .now()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    }
}

impl RoundResult {
    /// 全量计数：执行记录要能看出「11 个账号到底去哪了」。
    ///
    /// `summary()` 只列非零项，适合界面上那一行；日志要的是完整交代，
    /// 否则「无需签到」四个字背后是 11 个已签还是 11 个跳过，事后查不出来。
    pub fn detail(&self) -> String {
        format!(
            "{} · 扫描 {} · 到期 {} · 成功 {} · 已签 {} · 失败 {} · 等待 {} · 跳过 {}",
            self.message,
            self.scanned,
            self.due,
            self.success,
            self.already,
            self.failed,
            self.waiting,
            self.skipped
        )
    }

    pub fn summary(&self) -> String {
        let mut parts = vec![
            if self.new_day {
                "新的一天".to_string()
            } else {
                "同日轮询".to_string()
            },
            format!("{} 个账号", self.scanned),
        ];
        if self.due > 0 {
            parts.push(format!("可签 {}", self.due));
        }
        if self.success > 0 {
            parts.push(format!("成功 {}", self.success));
        }
        if self.already > 0 {
            parts.push(format!("已领 {}", self.already));
        }
        if self.waiting > 0 {
            parts.push(format!("等待窗口 {}", self.waiting));
        }
        if self.failed > 0 {
            parts.push(format!("失败 {}", self.failed));
        }
        if self.due == 0 && self.already == 0 && self.waiting == 0 {
            parts.push("无需签到".into());
        }
        parts.join(" · ")
    }
}

/// 触发方归一化：命令层收到的客户端传参只认已知值，未知/缺省一律按「手动」处理，
/// 不让任意字符串透传进动作日志。`auto` 是总览页积分过期时的自动补刷——
/// 不是用户点的，记成「手动」会误导排障。
pub fn normalize_trigger(raw: Option<&str>) -> &'static str {
    match raw {
        Some("auto") => "auto",
        Some("startup") => "startup",
        Some("tray") => "tray",
        Some("schedule") => "schedule",
        _ => "user",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_respects_platform_interval_and_never_blocks_first_query() {
        let now = 1_800_000_000_000i64;
        // 平台不设限：永远不跳过
        assert!(!throttled(0, Some(now), now));
        // 没查过：必须查（新账号不能一直等）
        assert!(!throttled(30, None, now));
        // 29 分钟前查过：跳过；31 分钟前：放行
        assert!(throttled(30, Some(now - 29 * 60_000), now));
        assert!(!throttled(30, Some(now - 31 * 60_000), now));
        // 边界：正好 30 分钟放行
        assert!(!throttled(30, Some(now - 30 * 60_000), now));
        // 时钟回拨（last_ok_at 在未来）按「刚查过」处理，不刷
        assert!(throttled(30, Some(now + 60_000), now));
    }

    fn row(date: &str, iid: &str, balance: f64) -> (String, String, f64) {
        (date.into(), iid.into(), balance)
    }

    #[test]
    fn flow_attributes_delta_to_start_day() {
        let points = aggregate_flow(
            vec![
                row("2026-11-01", "a", 100.0),
                row("2026-11-02", "a", 150.0),
                row("2026-11-03", "a", 120.0),
                row("2026-11-01", "b", 0.0),
                row("2026-11-02", "b", 0.0),
                row("2026-11-03", "b", 80.0),
            ],
            &std::collections::HashMap::new(),
            "2026-11-04",
        );
        assert_eq!(points.len(), 3);
        // 11-01 到 11-02 首笔：a 到账 50，b 没有变化
        assert_eq!(points[0].earned, 50.0);
        assert_eq!(points[0].spent, 0.0);
        // 11-02 到 11-03 首笔：a 消耗 30、b 到账 80 —— 跨账号求和会互相抵消，必须分开算
        assert_eq!(points[1].earned, 80.0);
        assert_eq!(points[1].spent, 30.0);
        assert_eq!(points[1].net, 50.0);
        assert_eq!(points[1].balance, 200.0);
        // 11-03 没有下一天首笔：只作为期末余额展示，不凭空算收支
        assert_eq!(points[2].earned, 0.0);
        assert_eq!(points[2].balance, 200.0);
    }

    #[test]
    fn flow_uses_latest_balance_for_today() {
        let latest = std::collections::HashMap::from([("a".to_string(), 150.0)]);
        let points = aggregate_flow(
            vec![row("2026-11-01", "a", 100.0), row("2026-11-02", "a", 120.0)],
            &latest,
            "2026-11-02",
        );
        // 昨天首笔 100 → 今天首笔 120；今天首笔 120 → 最新 150
        assert_eq!(points[0].earned, 20.0);
        assert_eq!(points[1].earned, 30.0);
        assert_eq!(points[1].balance, 150.0);
    }

    #[test]
    fn flow_ignores_first_day_and_float_noise() {
        let points = aggregate_flow(
            vec![
                row("2026-11-01", "a", 100.0),
                row("2026-11-02", "a", 100.0000000001),
            ],
            &std::collections::HashMap::new(),
            "2026-11-03",
        );
        assert_eq!(points[0].earned, 0.0, "浮点噪声不算变动");
        assert_eq!(points[1].earned, 0.0, "没有下一天也没有最新值，不算变动");
    }

    #[test]
    fn platform_flow_splits_software_instead_of_cancelling_out() {
        let rows = vec![
            row("2026-11-01", "workbuddy:u1", 100.0),
            row("2026-11-02", "workbuddy:u1", 150.0),
            row("2026-11-01", "zcode:u1", 80.0),
            row("2026-11-02", "zcode:u1", 30.0),
        ];
        let list = aggregate_platforms(
            rows,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            "2026-11-03",
        );
        assert_eq!(list.len(), 2);
        // 消耗降序：zcode 花了 50，WorkBuddy 是到账
        assert_eq!(list[0].platform, "zcode");
        assert_eq!(list[0].accounts, 1);
        assert_eq!(list[0].points[0].spent, 50.0);
        assert_eq!(list[1].platform, "workbuddy");
        assert_eq!(list[1].points[0].earned, 50.0);
    }

    /// 国内版与国际版都叫 workbuddy，但积分不互通：账目必须拆成两条。
    #[test]
    fn workbuddy_editions_are_separate_software() {
        let rows = vec![
            row("2026-11-01", "workbuddy:u1", 10.0),
            row("2026-11-02", "workbuddy:u1", 20.0),
            row("2026-11-01", "workbuddy:u2@intl", 300.0),
            row("2026-11-02", "workbuddy:u2@intl", 260.0),
        ];
        let list = aggregate_platforms(
            rows,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            "2026-11-03",
        );
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].platform, "workbuddy_intl");
        assert_eq!(list[0].points[0].spent, 40.0);
        assert_eq!(list[1].platform, "workbuddy");
    }

    #[test]
    fn platform_label_comes_from_account_table() {
        let view = crate::app::accounts::AccountView::new(
            crate::app::accounts::AccountIdentity {
                iid: "zcode:u1".into(),
                platform: "zcode".into(),
                platform_label: "ZCode".into(),
                account_id: "u1".into(),
                ..Default::default()
            },
            1,
        );
        let mut views = std::collections::HashMap::new();
        views.insert(view.iid.clone(), view);
        let list = aggregate_platforms(
            vec![
                row("2026-11-01", "zcode:u1", 10.0),
                row("2026-11-02", "zcode:u1", 5.0),
            ],
            &views,
            &std::collections::HashMap::new(),
            "2026-11-03",
        );
        assert_eq!(list[0].label, "ZCode");
    }

    #[test]
    fn trigger_normalization_whitelists_known_values() {
        assert_eq!(normalize_trigger(None), "user");
        assert_eq!(normalize_trigger(Some("user")), "user");
        assert_eq!(
            normalize_trigger(Some("auto")),
            "auto",
            "自动补刷必须与手动区分"
        );
        assert_eq!(normalize_trigger(Some("schedule")), "schedule");
        assert_eq!(normalize_trigger(Some("startup")), "startup");
        assert_eq!(normalize_trigger(Some("tray")), "tray");
        // 未知值按手动处理，不透传任意字符串进动作日志
        assert_eq!(normalize_trigger(Some("hacker")), "user");
        assert_eq!(normalize_trigger(Some("")), "user");
    }
}
