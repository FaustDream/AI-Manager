//! `Provider` trait 的 WorkBuddy 实现：扫描、签到状态、积分、旅行的接口层。

use super::crypto::open_field;
use super::endpoints::*;
use super::growth::{
    local_hhmm, normalize_travel_record, normalize_travel_status, positive_ts, travel_location,
};
use super::WorkBuddyProvider;
use crate::app::accounts::credential::{iso_from_ms, Credential};
use crate::app::accounts::AccountIdentity;
use crate::app::activity::{TravelConfig, TravelRecord, TravelStatus};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::Http;
use crate::app::providers::{Provider, ScannedAccount, Sink};
use crate::types::{trim_f64, ActionResult, CreditSnapshot, StatusResult};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashSet;

#[async_trait]
impl Provider for WorkBuddyProvider {
    fn platform(&self) -> &'static str {
        self.platform_key()
    }

    fn label(&self) -> &'static str {
        // 显示名只从产品表取：账号表、日志、界面上的名字必须完全一致
        if self.intl {
            crate::app::products::Product::WorkBuddyIntl.label()
        } else {
            crate::app::products::Product::WorkBuddyCn.label()
        }
    }

    fn scan(&self, sink: &mut Sink<'_>) -> Result<usize> {
        let mut count = 0;
        let mut seen: HashSet<String> = HashSet::new();
        for path in self.ticket_files() {
            // 单个票据坏掉不影响其他账号：这正是「一家一个模块」而不是巨型 match 的理由
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };
            let Ok(data) = serde_json::from_slice::<Value>(&raw) else {
                continue;
            };
            let Some(uid) = data.pointer("/account/uid").and_then(|v| v.as_str()) else {
                continue;
            };
            if uid.is_empty() || !seen.insert(uid.to_string()) {
                continue; // 同一账号的多份备份只取最新的一份（已按 mtime 倒序）
            }
            let auth = data.get("auth").cloned().unwrap_or_default();
            let token =
                open_field(auth.get("accessToken").unwrap_or(&Value::Null)).unwrap_or_default();
            if token.is_empty() {
                continue;
            }
            let name = open_field(data.pointer("/account/nickname").unwrap_or(&Value::Null))
                .ok()
                .filter(|s| !s.is_empty())
                // 按 char 取，不用字节切片：uid 是任意字符串，切在多字节中间会 panic
                .unwrap_or_else(|| {
                    format!("WorkBuddy {}", uid.chars().take(8).collect::<String>())
                });
            let expires_at = auth
                .get("expiresAt")
                .and_then(crate::app::timeutil::parse_ts);
            let account_id = if self.intl {
                format!("{uid}@intl")
            } else {
                uid.to_string()
            };

            let mut cred = Credential {
                token,
                refresh_token: open_field(auth.get("refreshToken").unwrap_or(&Value::Null))
                    .unwrap_or_default(),
                expires_at: expires_at.map(iso_from_ms).unwrap_or_default(),
                ..Default::default()
            };
            cred.put("uid", json!(uid));
            cred.put("name", json!(name));
            if let Some(domain) = auth.get("domain").filter(|v| v.is_string()) {
                cred.put("domain", domain.clone());
            }
            cred.put("edition", json!(self.edition()));
            // 账号自己的 UA：以后平台按客户端版本区分接口时，不用再改常量重扫
            cred.put("ua", json!(USER_AGENT));
            // 票据原文（含 sessionState）整份存进密文：切换时按字节写回，这是契约 8a 的由来
            cred.put(
                "ticket",
                json!(base64::engine::general_purpose::STANDARD.encode(&raw)),
            );
            cred.put(
                "ticket_file",
                json!(path.file_name().and_then(|n| n.to_str()).unwrap_or("")),
            );

            let identity = AccountIdentity {
                iid: format!("{}:{account_id}", self.platform()),
                platform: self.platform().to_string(),
                platform_label: self.label().to_string(),
                account_id,
                name,
                expires_at,
                source: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string(),
                // 国际版没有签到玩法：界面上不显示「待签」，也不参与批量签到
                requires_checkin: !self.intl,
                has_secret: true,
            };
            sink(ScannedAccount::new(identity, &cred)?)?;
            count += 1;
        }
        Ok(count)
    }

    fn current_id(&self) -> Option<String> {
        let uid = self.current_uid()?;
        Some(if self.intl {
            format!("{uid}@intl")
        } else {
            uid
        })
    }

    // ------------------------------------------------------------ P3：签到与积分

    /// 国际版没有签到玩法，只查积分
    fn requires_checkin(&self) -> bool {
        !self.intl
    }

    /// 资源包接口自带剩余量与到期时间，所以不写台账（契约 5）
    fn live_credit(&self) -> bool {
        true
    }

    async fn status(&self, cred: &Credential, http: &dyn Http) -> Result<StatusResult> {
        // 国际版不做签到：只读积分，别去请求 checkin-status
        if !self.requires_checkin() {
            let snapshot = self.credits(cred, http).await?;
            return Ok(StatusResult {
                checked_in: None,
                summary: snapshot.summary,
                balance: snapshot.balance,
                lots: snapshot.lots,
                next_at: None,
            });
        }

        // 新版看板优先。旧接口在活动未开放时返回 active=false、today_checked_in=false，
        // 看起来像「今天没签」，实际是接口根本没下发活动。
        let data = match self.activity_status(cred, http).await {
            Ok(Some(data)) => data,
            _ => {
                let body = self.post(cred, http, "checkin-status", &json!({})).await?;
                if body.get("code").and_then(|v| v.as_i64()) != Some(0) {
                    return Err(AppError::new(
                        ErrorKind::Provider,
                        body.get("msg")
                            .and_then(|v| v.as_str())
                            .unwrap_or("状态查询失败"),
                    ));
                }
                body.get("data").cloned().unwrap_or_else(|| json!({}))
            }
        };

        let lots = self.resource_lots(cred, http).await.ok();
        let mut parts: Vec<String> = Vec::new();
        let balance = match &lots {
            Some(lots) => {
                let total: f64 = lots.iter().map(|l| l.amount).sum();
                parts.push(format!("剩余 {} 积分", trim_f64(total)));
                Some(total)
            }
            None => match data.get("total_credits").and_then(|v| v.as_f64()) {
                Some(total) => {
                    parts.push(format!("累计 {} 积分", trim_f64(total)));
                    Some(total)
                }
                None => None,
            },
        };
        if let Some(streak) = data.get("streak_days").and_then(|v| v.as_i64()) {
            if streak > 0 {
                parts.push(format!("连续 {streak} 天"));
            }
        }
        let summary = if parts.is_empty() {
            "无可用积分".to_string()
        } else {
            parts.join("，")
        };

        let checked = data.get("today_checked_in").and_then(|v| v.as_bool());
        // active=false 且没签：活动没开，状态未知，不要显示成「待签」
        if data.get("active").and_then(|v| v.as_bool()) == Some(false) && checked != Some(true) {
            return Ok(StatusResult {
                checked_in: None,
                summary,
                balance,
                lots,
                next_at: None,
            });
        }
        Ok(StatusResult {
            checked_in: checked,
            summary,
            balance,
            lots,
            next_at: None,
        })
    }

    async fn credits(&self, cred: &Credential, http: &dyn Http) -> Result<CreditSnapshot> {
        // 只读资源包，不请求签到接口
        let lots = self.resource_lots(cred, http).await?;
        let total: f64 = lots.iter().map(|l| l.amount).sum();
        Ok(CreditSnapshot {
            summary: format!("剩余 {} 积分", trim_f64(total)),
            error: None,
            balance: Some(total),
            lots: Some(lots),
            quota: None,
        })
    }

    async fn checkin(&self, cred: &Credential, http: &dyn Http) -> Result<ActionResult> {
        if !self.requires_checkin() {
            return Ok(ActionResult {
                ok: false,
                message: "WorkBuddy 国际版无需签到".into(),
                granted: None,
                validity_days: None,
                already: false,
            });
        }
        let body = self.post(cred, http, "daily-checkin", &json!({})).await?;
        match body.get("code").and_then(|v| v.as_i64()) {
            Some(0) => {
                let data = body.get("data").cloned().unwrap_or_else(|| json!({}));
                let credit = data.get("credit").and_then(|v| v.as_f64());
                let days = data
                    .get("validity_days")
                    .or_else(|| data.get("expire_days"))
                    .and_then(|v| v.as_i64());
                Ok(ActionResult {
                    ok: true,
                    message: match credit {
                        Some(c) => format!("签到成功，+{} 积分", trim_f64(c)),
                        None => "签到成功".into(),
                    },
                    granted: credit,
                    validity_days: days,
                    already: false,
                })
            }
            // 10001 = 今日已签。这是「成功但没新发放」，不能当成今天刚签过
            Some(10001) => Ok(ActionResult {
                ok: true,
                message: "今日已签到".into(),
                granted: None,
                validity_days: None,
                already: true,
            }),
            other => {
                let code = other.unwrap_or(-1);
                let message = body
                    .get("msg")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("失败 code={code}"));
                Ok(ActionResult {
                    ok: false,
                    message: crate::app::providers::hints::with_hint(message, code),
                    granted: None,
                    validity_days: None,
                    already: false,
                })
            }
        }
    }

    // ------------------------------------------------------------ 猫猫旅行

    fn travel_supported(&self) -> bool {
        // 国际版没有这个活动；调用方在详情页据此隐藏卡片
        !self.intl
    }

    async fn travel_status(&self, cred: &Credential, http: &dyn Http) -> Result<TravelStatus> {
        let body = self.travel_call(cred, http, TRAVEL_STATUS, None).await?;
        Ok(normalize_travel_status(
            body.get("data").unwrap_or(&Value::Null),
        ))
    }

    async fn travel_config(&self, cred: &Credential, http: &dyn Http) -> Result<TravelConfig> {
        let body = self.travel_call(cred, http, TRAVEL_CONFIG, None).await?;
        let data = body.get("data").unwrap_or(&Value::Null);
        let locations = data
            .get("locations")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().filter_map(travel_location).collect())
            .unwrap_or_default();
        // Buddy 名字在另一个接口；读不到不影响地点列表
        let buddy_name = match self.travel_call(cred, http, TRAVEL_INFO, None).await {
            Ok(info) => info
                .pointer("/data/buddy/name")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            Err(_) => None,
        };
        Ok(TravelConfig {
            buddy_name,
            locations,
        })
    }

    async fn travel_records(
        &self,
        cred: &Credential,
        http: &dyn Http,
    ) -> Result<Vec<TravelRecord>> {
        let body = self.travel_call(cred, http, TRAVEL_RECORDS, None).await?;
        Ok(body
            .pointer("/data/records")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().map(normalize_travel_record).collect())
            .unwrap_or_default())
    }

    async fn travel_claim(&self, cred: &Credential, http: &dyn Http) -> Result<ActionResult> {
        let body = self
            .travel_call(cred, http, TRAVEL_CLAIM, Some(&json!({})))
            .await?;
        let data = body.get("data").unwrap_or(&Value::Null);
        let message = body
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("领取成功")
            .to_string();
        let reward = data
            .get("reward_credit")
            .or_else(|| data.get("credit"))
            .or_else(|| data.get("amount"))
            .and_then(|v| v.as_f64());
        let already = data
            .get("already")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || message.contains("已领取")
            || message.contains("重复");
        Ok(ActionResult {
            ok: true,
            message: match reward {
                Some(value) => format!("领取成功，+{} 积分", trim_f64(value)),
                None => message,
            },
            granted: reward,
            validity_days: None,
            already,
        })
    }

    async fn travel_depart(
        &self,
        cred: &Credential,
        http: &dyn Http,
        location_id: i64,
    ) -> Result<ActionResult> {
        let body = self
            .travel_call(
                cred,
                http,
                TRAVEL_DEPART,
                Some(&json!({ "location_id": location_id })),
            )
            .await?;
        let data = body.get("data").unwrap_or(&Value::Null);
        let name = data
            .pointer("/location/name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let arrive_at = data.get("arrive_at").and_then(positive_ts);
        let message = match (name.is_empty(), arrive_at) {
            (false, Some(at)) => format!("已派出到「{name}」，预计 {} 回来", local_hhmm(at)),
            (false, None) => format!("已派出到「{name}」"),
            (true, Some(at)) => format!("已派出，预计 {} 回来", local_hhmm(at)),
            (true, None) => "已派出".to_string(),
        };
        Ok(ActionResult {
            ok: true,
            message,
            granted: None,
            validity_days: None,
            already: false,
        })
    }
}
