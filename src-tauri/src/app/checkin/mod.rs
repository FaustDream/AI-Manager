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
use crate::app::error::{AppError, Result};
use crate::app::net::Http;
use crate::app::providers;
use crate::app::runtime::blocking;
use crate::app::task::CancelToken;
use crate::contracts::{CheckinStore, Ledger};
use crate::types::{ActionResult, CreditSnapshot, StatusResult};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;

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
/// 详情面板要的数据。**全部来自本地库**，点开一行才查，不打接口。
#[derive(Debug, Clone, Serialize)]
pub struct AccountDetail {
    pub balance: Option<f64>,
    /// 契约 6：今天与上一次有记录那天的差值
    pub today_delta: Option<f64>,
    pub lots: Vec<crate::types::CreditLot>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub next_at: Option<i64>,
    pub checked_in: bool,
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
        let db = self.db.clone();
        let wanted = iid.to_string();
        blocking(move || {
            let view = accounts::repo::list(&db)?
                .into_iter()
                .find(|v| v.iid == wanted)
                .ok_or_else(|| AppError::new("invalid", "账号不存在，可能已被删除"))?;
            let bytes = accounts::repo::secret_of(&db, &wanted)?.ok_or_else(|| {
                AppError::new("invalid", "这个账号没有保存凭据，请重新点「扫描客户端」")
            })?;
            Ok((view, accounts::credential::Credential::parse(&bytes)?))
        })
        .await?
    }

    fn provider_of(iid: &str) -> Result<Box<dyn providers::Provider>> {
        providers::for_iid(iid)
            .ok_or_else(|| AppError::new("unsupported", "这个平台的账号暂不支持该操作"))
    }

    /// 查签到状态。免签到的平台只查积分，不去「看」签到状态。
    pub async fn status(&self, iid: &str) -> Result<StatusResult> {
        let (view, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        let cooldown = provider.cooldown_hours();
        let requires = provider.requires_checkin();
        let result = if requires {
            provider.status(&cred, self.http.as_ref()).await?
        } else {
            let snapshot = provider.credits(&cred, self.http.as_ref()).await?;
            StatusResult {
                checked_in: None,
                summary: snapshot.summary,
                balance: snapshot.balance,
                lots: snapshot.lots,
                next_at: None,
            }
        };
        self.persist(iid, &result).await;
        if requires && result.next_at.is_none() {
            self.fallback_next_at(iid, cooldown).await;
        }
        let _ = view;
        Ok(result)
    }

    /// 只查积分（高频刷新走这条）。
    pub async fn credits(&self, iid: &str) -> Result<CreditSnapshot> {
        let (_, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        let snapshot = provider.credits(&cred, self.http.as_ref()).await?;
        if let Some(balance) = snapshot.balance {
            self.note_balance(iid, balance).await;
        }
        Ok(snapshot)
    }

    /// 批量刷积分：并发 3（与状态查询同口径），单账号失败不拖垮整批。
    /// 失败账号把错误文案装进 `summary`——界面拿它当「查询失败」渲染。
    pub async fn credits_batch(&self, iids: &[String]) -> Vec<(String, CreditSnapshot)> {
        let semaphore = Arc::new(Semaphore::new(3));
        let mut tasks = tokio::task::JoinSet::new();
        for iid in iids {
            let service = self.clone();
            let permit = semaphore.clone();
            let iid = iid.clone();
            tasks.spawn(async move {
                let _permit = permit.acquire_owned().await.ok();
                let snapshot = match service.credits(&iid).await {
                    Ok(snapshot) => snapshot,
                    Err(e) => CreditSnapshot {
                        summary: e.message,
                        balance: None,
                        lots: None,
                        quota: None,
                    },
                };
                (iid, snapshot)
            });
        }
        let mut out = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok(pair) = joined {
                out.push(pair);
            }
        }
        out
    }

    /// 定时积分刷新：全部有凭据的账号刷一遍余额。返回 (总数, 失败数)。
    pub async fn credits_round(&self) -> (usize, usize) {
        let iids: Vec<String> = match self.accounts().await {
            Ok(list) => list.into_iter().filter(|a| a.has_secret).map(|a| a.iid).collect(),
            Err(_) => return (0, 0),
        };
        let total = iids.len();
        if total == 0 {
            return (0, 0);
        }
        let out = self.credits_batch(&iids).await;
        // 失败的账号余额与配额都拿不到（错误文案在 summary 里）
        let failed = out
            .iter()
            .filter(|(_, s)| s.balance.is_none() && s.quota.is_none())
            .count();
        (total, failed)
    }

    /// 详情面板：只读本地库（余额、今日增减、批次、下次可签），一次都不打接口。
    pub async fn detail(&self, iid: &str) -> AccountDetail {
        let checked_in = self.marked_today(iid).await;
        let today = self.clock.today();
        let stamp = today.format("%Y-%m-%d").to_string();
        let db = self.db.clone();
        let key = iid.to_string();
        let (balance, delta, lots, next_at) = blocking(move || {
            let store = SqliteCheckin::new(&db);
            (
                store.balance_on(&key, &stamp),
                store::today_delta(&db, &key, today),
                store::lots_of(&db, &key, 10),
                store.next_at(&key).map(|at| at.timestamp_millis()),
            )
        })
        .await
        .unwrap_or((None, None, Vec::new(), None));
        AccountDetail {
            balance,
            today_delta: delta,
            lots,
            next_at,
            checked_in,
        }
    }

    /// 单账号签到。**幂等在这**：当日已签直接返回，绝不调接口。
    pub async fn checkin(&self, iid: &str) -> Result<ActionResult> {
        let (view, cred) = self.load(iid).await?;
        let provider = Self::provider_of(iid)?;
        if !provider.requires_checkin() {
            return Ok(ActionResult {
                ok: false,
                message: format!("{} 无需签到", view.platform_label),
                granted: None,
                validity_days: None,
                already: false,
            });
        }
        if self.marked_today(iid).await {
            return Ok(ActionResult {
                ok: true,
                message: "今日已签到（本工具的记录）".into(),
                granted: None,
                validity_days: None,
                already: true,
            });
        }

        let result = provider.checkin(&cred, self.http.as_ref()).await?;
        // 「本轮早领过」没有新发放，不能记成今天签过，留给下一轮再试
        if result.ok && !result.already {
            self.mark_today(iid).await?;
            if let Some(granted) = result.granted.filter(|g| *g > 0.0) {
                self.record_grant(iid, granted, result.validity_days, provider.live_credit())
                    .await;
            }
        }
        // 签到改变了余额：立刻查一次并落库，界面刷新账号列表就能看到，
        // 不用等下一轮轮询
        if result.ok {
            if let Ok(state) = self.status(iid).await {
                self.persist(iid, &state).await;
            }
        }
        Ok(result)
    }

    /// 批量签到：并发 3（平台风控上限），每个账号之间检查取消。
    pub async fn batch(
        &self,
        iids: &[String],
        cancel: Arc<CancelToken>,
        mut progress: impl FnMut(usize, usize),
    ) -> RoundResult {
        let total = iids.len();
        let mut round = RoundResult {
            scanned: total,
            ..Default::default()
        };
        let semaphore = Arc::new(Semaphore::new(3));
        let mut tasks = tokio::task::JoinSet::new();

        for iid in iids.iter().cloned() {
            if cancel.is_cancelled() {
                break;
            }
            let service = self.clone();
            let permit_sem = semaphore.clone();
            let cancel = cancel.clone();
            tasks.spawn(async move {
                let _permit = permit_sem.acquire_owned().await.ok();
                if cancel.is_cancelled() {
                    return (iid, Err(AppError::new("cancelled", "任务已取消")));
                }
                let result = service.checkin(&iid).await;
                (iid, result)
            });
        }

        let mut done = 0usize;
        while let Some(joined) = tasks.join_next().await {
            done += 1;
            progress(done, total);
            match joined {
                Ok((_, Ok(result))) => {
                    round.due += 1;
                    if result.ok && !result.already {
                        round.success += 1;
                    } else if result.ok {
                        round.already += 1;
                    } else {
                        round.failed += 1;
                    }
                }
                Ok((_, Err(e))) if e.is_cancelled() => round.skipped += 1,
                Ok((_, Err(_))) | Err(_) => round.failed += 1,
            }
        }
        round.at = self.now_text();
        round.message = round.summary();
        round
    }

    /// 跑一轮完整流程。`new_day` 靠 `daily_marks` 里的日期判断。
    /// 取消只在账号之间的安全点生效：正在路上的单个签到会跑完（接口幂等，重复也不会多签）。
    pub async fn run_round(&self, cancel: &CancelToken) -> RoundResult {
        let today = self.clock.today().format("%Y-%m-%d").to_string();
        let new_day = self.is_new_day(&today);
        let mut round = RoundResult {
            new_day,
            ..Default::default()
        };

        let accounts = match self.accounts().await {
            Ok(list) => list,
            Err(e) => {
                round.at = self.now_text();
                round.message = format!("读取账号失败：{e}");
                return round;
            }
        };
        round.scanned = accounts.len();
        let targets: Vec<AccountView> = accounts
            .into_iter()
            .filter(|a| a.requires_checkin && a.has_secret)
            .collect();

        // 用户点了取消：本轮直接作废，不计任何统计
        if cancel.is_cancelled() {
            round.at = self.now_text();
            round.message = "已取消".into();
            self.save_daily_state(&today, &round);
            return round;
        }

        // 第二步：逐个查接口状态（并发 3）
        let statuses = self.statuses_of(&targets).await;

        let mut due: Vec<String> = Vec::new();
        for account in &targets {
            let status = statuses.get(&account.iid);
            let remote = status.and_then(|s| s.checked_in);
            // 契约 1（resolve_checked）的生产接线：接口说了算，
            // 接口没给值（查询失败也在内）才看本地兜底
            let checked = match remote {
                Some(checked) => checked,
                None => self.marked_today(&account.iid).await,
            };
            if checked {
                // 接口说今天已签：写进去重表，后面就不必再问
                if let Err(e) = self.mark_today(&account.iid).await {
                    log_write_failure("写入去重标记失败", &account.platform, e);
                }
                round.skipped += 1;
                continue;
            }
            // 契约 2：当日已签成功的不重复调用——接口偶发说没签也不信
            if self.marked_today(&account.iid).await {
                round.skipped += 1;
                continue;
            }
            if let Some(next) = status.and_then(|s| s.next_at) {
                if next > self.clock.now() {
                    round.waiting += 1;
                    round.skipped += 1;
                    continue;
                }
            }
            due.push(account.iid.clone());
        }

        // 第三步：只对「今天还没签成功 + 现在确实可签」的账号调接口
        round.due = due.len();
        for iid in &due {
            // 取消安全点：每个账号之间都可以停，已签成的不回滚
            if cancel.is_cancelled() {
                break;
            }
            match self.checkin(iid).await {
                Ok(result) if result.ok && !result.already => round.success += 1,
                Ok(result) if result.ok => round.already += 1,
                Ok(_) => round.failed += 1,
                Err(_) => round.failed += 1,
            }
        }

        round.at = self.now_text();
        round.message = if cancel.is_cancelled() {
            "已取消".into()
        } else {
            round.summary()
        };
        self.save_daily_state(&today, &round);
        round
    }

    // ---------------------------------------------------------------- ZCode 活动领取
    // v1 的「领取」按钮：不是签到轮询的一部分（ZCode requires_checkin=false），
    // 是界面上单独的动作；领取会开浏览器过验证码。

    /// 可领取的活动套餐列表（preview 为主、marketing 补充）。
    pub async fn zcode_claimable(
        &self,
        iid: &str,
    ) -> Result<Vec<crate::app::providers::zclaim::ClaimPlan>> {
        let (_, cred) = self.load(iid).await?;
        crate::app::providers::zclaim::claimable(&cred, self.http.as_ref(), None).await
    }

    /// 领取一个套餐：开浏览器过验证码，拿到参数自动提交。3007 不自动重试。
    pub async fn zcode_claim(
        &self,
        iid: &str,
        plan_id: &str,
    ) -> Result<crate::app::providers::zclaim::ClaimOutcome> {
        let (view, cred) = self.load(iid).await?;
        let outcome =
            crate::app::providers::zclaim::claim(&cred, self.http.as_ref(), None, plan_id).await?;
        let name = if outcome.name.is_empty() {
            plan_id
        } else {
            &outcome.name
        };
        crate::app::log::log(
            crate::app::log::Level::Info,
            "checkin",
            format!("ZCode 活动领取成功：{} · {name}", view.name),
        );
        Ok(outcome)
    }

    /// 自动领取一轮（v1 每 15 分钟，开关 `zcode_auto_claim` 默认关）：
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
                    account.name,
                    if outcome.name.is_empty() {
                        label
                    } else {
                        outcome.name
                    }
                ),
                Err(e) => format!("{}：领取失败（{}）", account.name, e.message),
            };
            return Some(line);
        }
        None
    }

    /// 免签到账号也要补一次积分：否则整表重画后它们的余额永远是「—」（契约 21）。
    pub async fn refresh_credits(&self) -> usize {
        let Ok(accounts) = self.accounts().await else {
            return 0;
        };
        let semaphore = Arc::new(Semaphore::new(3));
        let mut tasks = tokio::task::JoinSet::new();
        for account in accounts.into_iter().filter(|a| a.has_secret) {
            let service = self.clone();
            let permit = semaphore.clone();
            tasks.spawn(async move {
                let _permit = permit.acquire_owned().await.ok();
                if account.requires_checkin {
                    service.status(&account.iid).await.is_ok()
                } else {
                    service.credits(&account.iid).await.is_ok()
                }
            });
        }
        let mut ok = 0;
        while let Some(joined) = tasks.join_next().await {
            if matches!(joined, Ok(true)) {
                ok += 1;
            }
        }
        ok
    }

    // ---------------------------------------------------------------- 内部

    async fn accounts(&self) -> Result<Vec<AccountView>> {
        let db = self.db.clone();
        blocking(move || accounts::repo::list(&db)).await?
    }

    /// 逐个查接口状态（并发 3）。查询失败的账号不进结果表，
    /// 由调用方按契约 1 用本地记录兜底。
    async fn statuses_of(
        &self,
        targets: &[AccountView],
    ) -> std::collections::HashMap<String, StatusResult> {
        let semaphore = Arc::new(Semaphore::new(3));
        let mut tasks = tokio::task::JoinSet::new();
        for account in targets {
            let service = self.clone();
            let permit = semaphore.clone();
            let iid = account.iid.clone();
            tasks.spawn(async move {
                let _permit = permit.acquire_owned().await.ok();
                (iid.clone(), service.status(&iid).await)
            });
        }
        let mut out = std::collections::HashMap::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((iid, Ok(status))) => {
                    out.insert(iid, status);
                }
                Ok((_, Err(_))) => {}
                Err(e) => crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "checkin",
                    format!("状态查询任务崩溃：{e}"),
                ),
            }
        }
        out
    }

    async fn persist(&self, iid: &str, status: &StatusResult) {
        if let Some(balance) = status.balance {
            self.note_balance(iid, balance).await;
        }
        self.note_next_at(iid, status.next_at).await;
    }

    async fn note_balance(&self, iid: &str, balance: f64) {
        let db = self.db.clone();
        let clock = self.clock.clone();
        let iid = iid.to_string();
        let scope = platform_scope(&iid);
        match blocking(move || {
            SqliteCheckin::new(&db).note_balance(&iid, balance, clock.as_ref())
        })
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
                live_credit, &iid, granted, validity_days, clock.as_ref(),
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
