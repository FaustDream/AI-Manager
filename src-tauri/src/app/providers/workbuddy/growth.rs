//! 成长中心与旅行：每日活跃、平台奖励积分、旅行的数据面方法与解析。

use super::endpoints::*;
use super::WorkBuddyProvider;
use crate::app::accounts::credential::Credential;
use crate::app::activity::{TravelLocation, TravelRecord, TravelState, TravelStatus};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::types::CreditLot;
use serde::Serialize;
use serde_json::{json, Value};

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct RewardCredits {
    /// 服务端口径的总积分（`TotalDosage`）
    pub total: f64,
    /// 按包名聚合：还剩多少、一共发过多少、几批、最晚什么时候到期
    pub packages: Vec<RewardPackage>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub refreshed_at: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct RewardPackage {
    pub name: String,
    /// 本包剩余
    pub remain: f64,
    /// 本包总量（发放时的容量）
    pub size: f64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: i64,
    /// 最晚一批的到期时刻（毫秒）；国际版字段不可靠时为 null
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires_at: Option<i64>,
}

impl WorkBuddyProvider {
    pub(crate) fn api_base(&self) -> &'static str {
        if self.intl {
            API_INTL
        } else {
            API_CN
        }
    }

    pub(crate) fn headers(&self, cred: &Credential) -> Vec<(String, String)> {
        vec![
            ("Authorization".into(), format!("Bearer {}", cred.token)),
            (
                "X-User-Id".into(),
                cred.meta_str("uid").unwrap_or("").to_string(),
            ),
            (
                "X-Domain".into(),
                cred.meta_str("domain").unwrap_or("").to_string(),
            ),
            ("Content-Type".into(), "application/json".into()),
            ("User-Agent".into(), cred.user_agent(USER_AGENT)),
        ]
    }

    // ------------------------------------------------------------ 成长中心

    /// 成长任务列表（只读）：每条任务的领取条件与完成状态都由服务端判定。
    pub async fn growth_tasks(&self, cred: &Credential, http: &dyn Http) -> Result<Value> {
        self.travel_call(cred, http, GROWTH_TASKS, None).await
    }

    /// 活跃地图（只读）：`month` 形如 `2026-11`，返回该月每天的活跃标记。
    pub async fn growth_heatmap(
        &self,
        cred: &Credential,
        http: &dyn Http,
        month: &str,
    ) -> Result<Value> {
        let path = format!("{GROWTH_HEATMAP}?month={month}");
        self.travel_call(cred, http, &path, None).await
    }

    /// 连续活跃天数（只读）。
    pub async fn growth_streak(&self, cred: &Credential, http: &dyn Http) -> Result<Value> {
        self.travel_call(cred, http, GROWTH_STREAK, None).await
    }

    /// 核心 API（`copilot.tencent.com`）调用：客户端 bundle 里 routeId=`core-api`，
    /// 与 `www.workbuddy.cn` 的活动域名不同，鉴权头是同一套。
    pub async fn core_call(
        &self,
        cred: &Credential,
        http: &dyn Http,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let text = self.core_text(cred, http, path, body).await?;
        serde_json::from_str(&text).map_err(|_| {
            AppError::new(
                ErrorKind::Provider,
                format!(
                    "{path} 返回的不是 JSON：{}",
                    text.chars().take(120).collect::<String>()
                ),
            )
        })
    }

    /// 同 `core_call`，但返回原始文本（对话接口只支持 SSE 流，不能当 JSON 解析）。
    pub async fn core_text(
        &self,
        cred: &Credential,
        http: &dyn Http,
        path: &str,
        body: Option<&Value>,
    ) -> Result<String> {
        let url = format!("{CORE_BASE}{path}");
        let mut req = match body {
            Some(_) => HttpRequest::post(url.as_str()),
            None => HttpRequest::get(url.as_str()),
        };
        req.headers = self.headers(cred);
        if let Some(body) = body {
            req.body = Some(body.to_string());
        }
        let resp = http.send(req).await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(AppError::new(
                ErrorKind::AuthExpired,
                "HTTP 401/403（登录态可能过期）",
            ));
        }
        if resp.status >= 400 {
            // 带上服务端原话：这条路是逆向出来的，400/422 的具体原因比状态码有用得多
            let hint: String = resp.body.chars().take(200).collect();
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("HTTP {}（{path}）：{hint}", resp.status),
            ));
        }
        Ok(resp.body)
    }

    /// 旅行接口统一入口：GET/POST + 鉴权头 + 业务码归一化。
    /// 写操作（claim/depart）也只经过这里，避免再写一套状态码判断。
    pub(crate) async fn travel_call(
        &self,
        cred: &Credential,
        http: &dyn Http,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let url = format!("{TRAVEL_BASE}{path}");
        let mut req = match body {
            Some(_) => HttpRequest::post(url.as_str()),
            None => HttpRequest::get(url.as_str()),
        };
        req.headers = self.headers(cred);
        if let Some(body) = body {
            req.body = Some(body.to_string());
        }
        let resp = http.send(req).await?;
        match resp.status {
            401 | 403 => {
                return Err(AppError::new(
                    ErrorKind::AuthExpired,
                    "HTTP 401/403（登录态可能过期）",
                ))
            }
            429 => {
                return Err(AppError::new(
                    ErrorKind::RateLimited,
                    "平台限流（HTTP 429）",
                ))
            }
            status if status >= 500 => {
                return Err(AppError::new(ErrorKind::Provider, format!("HTTP {status}")))
            }
            status if status >= 400 => {
                return Err(AppError::new(ErrorKind::Provider, format!("HTTP {status}")))
            }
            _ => {}
        }
        let body = resp.json()?;
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
        if code != 0 {
            let message = body
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("请求失败");
            let kind = if message.contains("未解锁")
                || message.contains("未开放")
                || message.contains("已结束")
            {
                ErrorKind::ActivityClosed
            } else {
                ErrorKind::Provider
            };
            return Err(AppError::new(
                kind,
                crate::app::providers::hints::with_hint(format!("{message}（code={code}）"), code),
            ));
        }
        Ok(body)
    }

    pub(crate) async fn post(
        &self,
        cred: &Credential,
        http: &dyn Http,
        path: &str,
        body: &Value,
    ) -> Result<Value> {
        let url = format!("{}/{path}", self.api_base());
        let req = HttpRequest::post(url)
            .headers(&self.headers(cred))
            .json_body(body);
        let resp = http.send(req).await?;
        if resp.status == 401 {
            return Err(AppError::new(
                ErrorKind::AuthExpired,
                "HTTP 401（登录态可能过期）",
            ));
        }
        if resp.status >= 500 {
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("HTTP {}", resp.status),
            ));
        }
        resp.json()
    }

    /// 新版签到看板。活动未开放时返回 404 / 4xx，这一层只表示「没读到」。
    pub(crate) async fn activity_status(
        &self,
        cred: &Credential,
        http: &dyn Http,
    ) -> Result<Option<Value>> {
        let url = format!("{}/checkin-activity-status", self.api_base());
        let req = HttpRequest::post(url)
            .headers(&self.headers(cred))
            .json_body(&json!({}));
        let resp = http.send(req).await?;
        if resp.status == 401 {
            return Err(AppError::new(
                ErrorKind::AuthExpired,
                "HTTP 401（登录态可能过期）",
            ));
        }
        if resp.status >= 400 {
            return Ok(None);
        }
        let Ok(body) = resp.json() else {
            return Ok(None);
        };
        if body.get("code").and_then(|v| v.as_i64()) == Some(0) {
            return Ok(body.get("data").filter(|v| v.is_object()).cloned());
        }
        Ok(None)
    }

    /// 平台奖励积分明细（个人中心「套餐与积分」那一页的数据）。
    ///
    /// 与 `resource_lots` 的分工：那个只给「还能用的批次」用于算余额，
    /// 这个按包名聚合，回答「这些积分是哪来的、还剩多少、什么时候到期」。
    pub async fn reward_credits(
        &self,
        cred: &Credential,
        http: &dyn Http,
    ) -> Result<RewardCredits> {
        let body = json!({
            "PageNumber": 1,
            "PageSize": 200,
            "ProductCode": "p_tcaca",
            "Status": [0, 3],
        });
        let payload = self.post(cred, http, "get-user-resource", &body).await?;
        let data = payload
            .pointer("/data/Response/Data")
            .or_else(|| payload.pointer("/data/Data"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let items = data
            .get("Accounts")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut grouped: std::collections::BTreeMap<String, RewardPackage> =
            std::collections::BTreeMap::new();
        for item in &items {
            let remain = [
                "CycleCapacityRemainPrecise",
                "CycleCapacityRemain",
                "CapacityRemainPrecise",
                "CapacityRemain",
            ]
            .iter()
            .find_map(|key| item.get(*key).and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
            // 用完了的包不占版面：列表里 66 个包，真正还有余额的只有一半
            if remain <= 0.0 {
                continue;
            }
            let size = [
                "CycleCapacitySizePrecise",
                "CycleCapacitySize",
                "CapacitySizePrecise",
                "CapacitySize",
            ]
            .iter()
            .find_map(|key| item.get(*key).and_then(|v| v.as_f64()))
            .unwrap_or(remain);
            let expires = item
                .get("DeductionEndTime")
                .and_then(dt_of)
                .or_else(|| item.get("CycleEndTime").and_then(local_text))
                .or_else(|| item.get("ExpiredTime").and_then(dt_of))
                .map(|dt| dt.timestamp_millis());
            let name = item
                .get("PackageName")
                .and_then(|v| v.as_str())
                .unwrap_or("积分包")
                .to_string();
            let entry = grouped.entry(name.clone()).or_insert(RewardPackage {
                name,
                remain: 0.0,
                size: 0.0,
                count: 0,
                expires_at: None,
            });
            entry.remain += remain;
            entry.size += size;
            entry.count += 1;
            // 同一包名可能有多批，取最近到期的那批（用户关心「最晚能用到什么时候」）
            entry.expires_at = match (entry.expires_at, expires) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (None, b) => b,
                (a, None) => a,
            };
        }

        let mut packages: Vec<RewardPackage> = grouped.into_values().collect();
        packages.sort_by(|a, b| {
            b.remain
                .partial_cmp(&a.remain)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(RewardCredits {
            total: data
                .get("TotalDosage")
                .and_then(|v| v.as_f64())
                .unwrap_or_else(|| packages.iter().map(|p| p.remain).sum()),
            packages,
            refreshed_at: chrono::Local::now().timestamp_millis(),
        })
    }

    /// 资源包批次。**不要带 `PackageEndTimeRange`**：部分账号的活跃资源包不在那个
    /// 区间里，服务端会只返回历史包，看起来就是「查不到积分」。
    pub(crate) async fn resource_lots(
        &self,
        cred: &Credential,
        http: &dyn Http,
    ) -> Result<Vec<CreditLot>> {
        let body = json!({
            "PageNumber": 1,
            "PageSize": 200,
            "ProductCode": "p_tcaca",
            "Status": [0, 3],
        });
        let payload = self.post(cred, http, "get-user-resource", &body).await?;
        let data = payload
            .pointer("/data/Response/Data")
            .or_else(|| payload.pointer("/data/Data"))
            .cloned()
            .unwrap_or_else(|| json!({}));

        let now = chrono::Local::now();
        let mut lots: Vec<CreditLot> = data
            .get("Accounts")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|item| {
                // Cycle* 是本周期剩余，Capacity* 是整个套餐周期的总容量：只有「本期已
                // 消耗」时两者才不同（体验版常见 Capacity 500 / CycleRemain 0）。
                // 所以必须先取 Cycle，字段缺失才回退 Capacity，否则会把用完的当剩余。
                let amount = [
                    "CycleCapacityRemainPrecise",
                    "CapacityRemainPrecise",
                    "CycleCapacityRemain",
                    "CapacityRemain",
                ]
                .iter()
                .find_map(|key| item.get(*key).and_then(|v| v.as_f64()))
                .filter(|v| *v > 0.0)?;
                let expires = if self.intl {
                    // 国际版的资源包字段里没有可靠的「到期」：CycleEndTime 是周期重置时间，
                    // 拿它当到期会显示一个错的日期。宁可空着，也不猜。
                    None
                } else {
                    item.get("DeductionEndTime")
                        .and_then(dt_of)
                        .or_else(|| item.get("CycleEndTime").and_then(local_text))
                        .or_else(|| item.get("ExpiredTime").and_then(dt_of))
                };
                if expires.map(|e| e <= now).unwrap_or(false) {
                    return None;
                }
                Some(CreditLot {
                    amount,
                    expires_at: expires,
                    granted_at: None,
                    lot_key: String::new(),
                    label: item
                        .get("PackageName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("积分包")
                        .to_string(),
                })
            })
            .collect();
        lots.sort_by_key(|a| a.expires_at);
        Ok(lots)
    }
}

pub(crate) fn dt_of(value: &Value) -> Option<chrono::DateTime<chrono::Local>> {
    crate::app::timeutil::parse_ts(value)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.with_timezone(&chrono::Local))
}

/// 资源包里的 `CycleEndTime` 是本地时间字符串 `2026-11-01 10:00:00`（无时区）。
pub(crate) fn local_text(value: &Value) -> Option<chrono::DateTime<chrono::Local>> {
    let text = value.as_str()?;
    let naive = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").ok()?;
    naive.and_local_timezone(chrono::Local).single()
}

pub(crate) fn travel_location(value: &Value) -> Option<TravelLocation> {
    let obj = value.as_object()?;
    let id = obj.get("id").and_then(|v| v.as_i64())?;
    Some(TravelLocation {
        id,
        code: obj
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        description: obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        duration_hours: obj.get("duration_hours").and_then(|v| v.as_i64()),
        duration_hours_min: obj.get("duration_hours_min").and_then(|v| v.as_i64()),
        duration_hours_max: obj.get("duration_hours_max").and_then(|v| v.as_i64()),
        reward_credit_min: obj.get("reward_credit_min").and_then(|v| v.as_f64()),
        reward_credit_max: obj.get("reward_credit_max").and_then(|v| v.as_f64()),
    })
}

pub(crate) fn normalize_travel_status(data: &Value) -> TravelStatus {
    let daily_limit_reached = data
        .get("daily_limit_reached")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let raw = data
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let state = match (raw, daily_limit_reached) {
        ("idle", true) | ("limit", _) => TravelState::Limit,
        ("idle", false) => TravelState::Idle,
        ("traveling", _) => TravelState::Traveling,
        ("arrived", _) => TravelState::Arrived,
        _ => TravelState::Unknown,
    };

    TravelStatus {
        state: Some(state),
        location: data.get("location").and_then(travel_location),
        arrive_at: data.get("arrive_at").and_then(positive_ts),
        depart_at: data.get("depart_at").and_then(positive_ts),
        server_now: data.get("server_now").and_then(positive_ts),
        reward_credit: data.get("reward_credit").and_then(|v| v.as_f64()),
        duration_hours: data.get("duration_hours").and_then(|v| v.as_i64()),
        daily_limit_reached,
        letter: data
            .pointer("/letter/text")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        buddy_name: None,
        record_id: data.get("record_id").and_then(|v| v.as_i64()),
    }
}

pub(crate) fn normalize_travel_record(value: &Value) -> TravelRecord {
    TravelRecord {
        record_id: value.get("record_id").and_then(|v| v.as_i64()),
        travel_date: value
            .get("travel_date")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        location_name: value
            .pointer("/location/name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        reward_credit: value.get("reward_credit").and_then(|v| v.as_f64()),
        depart_at: value.get("depart_at").and_then(positive_ts),
        arrive_at: value.get("arrive_at").and_then(positive_ts),
        claimed_at: value.get("claimed_at").and_then(positive_ts),
        letter: value
            .pointer("/letter/text")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

/// 0 在这套接口里表示「没有时间」（idle 状态的 arrive_at 就是 0），
/// 不能当成 1970-01-01，否则界面会显示一个看起来很真的假日期。
pub(crate) fn positive_ts(value: &Value) -> Option<i64> {
    crate::app::timeutil::parse_ts(value).filter(|ms| *ms > 0)
}

pub(crate) fn local_hhmm(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}
