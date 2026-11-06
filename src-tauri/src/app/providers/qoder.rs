//! Qoder 账号扫描与登录态写回：读写客户端的 Chromium 信封。
//!
//! 两个版本共用一套逻辑，只有 profile 目录不同：
//!   - 国内版 `%APPDATA%\com.qodercn.app.stable`
//!   - 国际版 `%APPDATA%\com.qoder.app.stable`
//!
//! 文件结构：`Local State` 里的 `os_crypt.encrypted_key`（DPAPI 保护的主密钥）
//! + `auth.v1.dat`（`v10` 信封的 AES-256-GCM 密文）。解出来的 JSON 里有
//!   `token` / `refreshToken` / `expiresAt` / `user{id,name,email,phone}`。
//!
//! **写回只要重封 `auth.v1.dat`**：Qoder 启动时只认这个文件里的 token，
//! 不像 WorkBuddy 那样还要换整份 Chromium 登录态。

use super::{Provider, ScannedAccount, Sink, SwitchOutcome};
use crate::app::accounts::credential::{iso_from_ms, Credential};
use crate::app::accounts::AccountIdentity;
use crate::app::clock::{Clock, SystemClock};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::app::{fsx, process, secret};
use crate::types::{trim_f64, ActionResult, CreditLot, CreditSnapshot, StatusResult};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct QoderProvider {
    pub intl: bool,
    /// 可注入：测试指向临时目录，生产指向 `%APPDATA%`
    profile: Option<PathBuf>,
    /// 进程探测的注入点：`None` = 真去查全局进程表（生产路径）
    alive: Option<bool>,
    clock: Arc<dyn Clock>,
    #[cfg(test)]
    request_headers: Option<Vec<(String, String)>>,
}

impl QoderProvider {
    pub fn new(intl: bool) -> Self {
        Self {
            intl,
            profile: default_profile(intl),
            alive: None,
            clock: Arc::new(SystemClock),
            #[cfg(test)]
            request_headers: None,
        }
    }

    /// 指定 profile 目录（测试与「额外来源」用）
    pub fn at(intl: bool, profile: PathBuf) -> Self {
        Self {
            intl,
            profile: Some(profile),
            alive: None,
            clock: Arc::new(SystemClock),
            #[cfg(test)]
            request_headers: None,
        }
    }

    /// 测试专用：固定「客户端是否在跑」，不碰真实进程表。
    /// 不注入的话「客户端开着就红」的用例会在开发机上随机红绿，CI 上必炸。
    pub fn assume_alive(mut self, value: bool) -> Self {
        self.alive = Some(value);
        self
    }

    /// 客户端是否在运行。生产路径走真实探测，测试由 `assume_alive` 覆盖。
    fn client_running(&self) -> bool {
        self.alive
            .unwrap_or_else(|| process::process_running(self.process_names()))
    }

    pub fn platform_key(&self) -> &'static str {
        if self.intl {
            "qoder_intl"
        } else {
            "qoder"
        }
    }

    pub fn profile_dir(&self) -> Option<&Path> {
        self.profile.as_deref()
    }

    pub fn process_names(&self) -> &'static [&'static str] {
        if self.intl {
            &["Qoder.exe"]
        } else {
            &["Qoder CN.exe"]
        }
    }

    /// 解开登录态并返回 (JSON, 主密钥)。缺文件返回 Ok(None)。
    fn read_auth(&self) -> Result<Option<(Value, Key<Aes256Gcm>)>> {
        let Some(profile) = self.profile_dir() else {
            return Ok(None);
        };
        let auth_file = profile.join("auth.v1.dat");
        let local_state = profile.join("Local State");
        if !auth_file.exists() || !local_state.exists() {
            return Ok(None);
        }
        let key = secret::chromium_key(&local_state)?;
        let blob = std::fs::read(&auth_file)?;
        let plain = secret::chromium_open(&key, &blob)?;
        let auth: Value = serde_json::from_slice(&plain)
            .map_err(|e| AppError::new(ErrorKind::Invalid, format!("登录态不是 JSON：{e}")))?;
        Ok(Some((auth, key)))
    }

    /// 把一份登录态重新封成 `v10` 信封。
    fn seal(key: &Key<Aes256Gcm>, plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut nonce)
            .map_err(|e| AppError::new(ErrorKind::Crypto, format!("取随机数失败：{e}")))?;
        let ciphertext = Aes256Gcm::new(key)
            .encrypt(aes_gcm::Nonce::from_slice(&nonce), plain)
            .map_err(|_| AppError::new(ErrorKind::Crypto, "AES-GCM 加密失败"))?;
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        Ok(blob)
    }
}

/// 把凭据包还原成客户端认识的 `auth.v1.dat` 内容。
///
/// `current` 是切换前客户端里的那份：`schemaVersion` / `authenticationContext` /
/// 头像等字段客户端自己写的，没有对应的凭据来源，只能沿用；
/// 但 `user` 的身份字段只在**同一个账号**时才沿用——否则会把 A 的邮箱写给 B。
pub fn build_auth(
    cred: &Credential,
    account_id: &str,
    name: &str,
    current: Option<&Value>,
) -> Value {
    let now = chrono::Local::now();
    let expires = if cred.expires_at.trim().is_empty() {
        // 没有到期时间时给一小时：客户端据此触发 refresh，比空串安全
        (now + chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    } else {
        cred.expires_at.clone()
    };
    let refresh_expires = cred
        .meta_str("refresh_expires_at")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            (now + chrono::Duration::days(30)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });

    let old_user = current.and_then(|c| c.get("user"));
    let same_user = old_user
        .and_then(|u| u.get("id"))
        .and_then(|v| v.as_str())
        .map(|id| id == account_id)
        .unwrap_or(false);

    let mut user = Map::new();
    user.insert("id".into(), json!(account_id));
    user.insert("name".into(), json!(name));
    user.insert("email".into(), json!(cred.meta_str("email").unwrap_or("")));
    user.insert(
        "phone".into(),
        json!(if same_user {
            old_user.and_then(|u| u.get("phone")).cloned()
        } else {
            None
        }),
    );
    user.insert(
        "avatarUrl".into(),
        json!(if same_user {
            old_user.and_then(|u| u.get("avatarUrl")).cloned()
        } else {
            None
        }),
    );

    json!({
        "schemaVersion": current.and_then(|c| c.get("schemaVersion")).cloned().unwrap_or(json!(1)),
        "token": cred.token,
        "refreshToken": cred.refresh(),
        "expiresAt": expires,
        "refreshTokenExpiresAt": refresh_expires,
        "user": user,
        "authenticationContext": current.and_then(|c| c.get("authenticationContext")).cloned(),
    })
}

/// 切换登录。写前备份、写后验读，验不过就回滚。
pub fn switch_to(
    provider: &QoderProvider,
    account_id: &str,
    name: &str,
    cred: &Credential,
) -> Result<SwitchOutcome> {
    let Some(profile) = provider.profile_dir() else {
        return Err(AppError::new(ErrorKind::Io, "没有找到 Qoder 的客户端目录"));
    };
    if !profile.exists() {
        return Err(AppError::new(ErrorKind::Io, "没有找到 Qoder 的客户端目录"));
    }
    // 先判凭据：缺 refresh token 时写回去只会立刻被登出，不必让用户关客户端
    if cred.refresh().is_empty() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "这个账号没有 refresh token，写回去会被立刻登出",
        ));
    }
    if provider.client_running() {
        return Err(AppError::new(
            ErrorKind::ClientRunning,
            "请先关闭 Qoder 客户端再切换",
        ));
    }

    let path = profile.join("auth.v1.dat");
    let local_state = profile.join("Local State");
    if !path.exists() || !local_state.exists() {
        return Err(AppError::new(ErrorKind::Io, "没有找到 Qoder 的登录态文件"));
    }

    // 先解出当前内容：既是模板，也是回滚依据
    let current = match secret::chromium_key(&local_state) {
        Ok(key) => std::fs::read(&path)
            .ok()
            .and_then(|blob| secret::chromium_open(&key, &blob).ok())
            .and_then(|plain| serde_json::from_slice::<Value>(&plain).ok()),
        Err(_) => None,
    };

    if current
        .as_ref()
        .and_then(|c| c.pointer("/user/id"))
        .and_then(|v| v.as_str())
        == Some(account_id)
    {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "这个账号就是客户端当前登录的账号，不需要切换",
        ));
    }

    let auth = build_auth(cred, account_id, name, current.as_ref());
    let key = secret::chromium_key(&local_state)?;
    let sealed = QoderProvider::seal(&key, &serde_json::to_vec(&auth)?)?;

    // 原文件读进内存再写：写后验读失败要能原样盖回去。
    // 这是**本次写入**的回滚依据，不落盘——切号不再留历史备份。
    let before = std::fs::read(&path).ok();
    fsx::write_atomic(&path, &sealed)?;

    // 写后验读：能解回、且 user.id 是目标账号，才算真的写成功
    let ok = std::fs::read(&path)
        .ok()
        .and_then(|blob| secret::chromium_open(&key, &blob).ok())
        .and_then(|plain| serde_json::from_slice::<Value>(&plain).ok())
        .map(|v| v.pointer("/user/id").and_then(|x| x.as_str()) == Some(account_id))
        .unwrap_or(false);
    if !ok {
        match before {
            Some(bytes) => fsx::write_atomic(&path, &bytes)?,
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
        return Err(AppError::new(
            ErrorKind::WriteFailed,
            "写入后校验失败，已回滚到切换前的登录态",
        ));
    }

    Ok(SwitchOutcome {
        message: format!("{} 已切换到 {name}", provider.label()),
    })
}

#[async_trait]
impl Provider for QoderProvider {
    fn platform(&self) -> &'static str {
        self.platform_key()
    }

    /// Qoder 是唯一按「上次签到 + 24h」冷却的平台（契约 4 的第二套口径）。
    fn cooldown_hours(&self) -> u32 {
        24
    }

    fn label(&self) -> &'static str {
        if self.intl {
            "Qoder 国际版"
        } else {
            "Qoder CN"
        }
    }

    fn scan(&self, sink: &mut Sink<'_>) -> Result<usize> {
        let Some((auth, _key)) = self.read_auth()? else {
            return Ok(0);
        };
        let token = auth
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "登录态里没有 token"))?;
        let user = auth.get("user").cloned().unwrap_or_default();
        let name = user
            .get("name")
            .or_else(|| user.get("email"))
            .and_then(|v| v.as_str())
            .unwrap_or("Qoder 用户")
            .to_string();
        // 没有 user.id 时用 token 末 16 位兜底（与 v1 一致，避免把整串 token 当 id）
        let account_id = user
            .get("id")
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| tail(token, 16));
        let expires_at = auth
            .get("expiresAt")
            .and_then(crate::app::timeutil::parse_ts);

        let mut cred = Credential {
            token: token.to_string(),
            refresh_token: auth
                .get("refreshToken")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            expires_at: expires_at.map(iso_from_ms).unwrap_or_default(),
            ..Default::default()
        };
        cred.put("uid", json!(account_id));
        cred.put("name", json!(name));
        // 账号自己的 UA：与 Provider 请求头同一口径，旧凭据没有时回退常量
        cred.put("ua", json!("Qoder"));
        for key in ["email", "phone"] {
            if let Some(v) = user.get(key).filter(|v| !v.is_null()) {
                cred.put(key, v.clone());
            }
        }
        if let Some(v) = auth.get("refreshTokenExpiresAt") {
            cred.put("refresh_expires_at", v.clone());
        }
        if let Some(v) = auth.get("authenticationContext") {
            cred.put("authentication_context", v.clone());
        }

        let identity = AccountIdentity {
            iid: format!("{}:{account_id}", self.platform()),
            platform: self.platform().to_string(),
            platform_label: self.label().to_string(),
            account_id,
            name,
            expires_at,
            // 只留文件名：绝对路径里有用户名，属于不该出界的隐私
            source: "auth.v1.dat".to_string(),
            requires_checkin: true,
            has_secret: true,
        };
        sink(ScannedAccount::new(identity, &cred)?)?;
        Ok(1)
    }

    fn current_id(&self) -> Option<String> {
        let (auth, _) = self.read_auth().ok()??;
        auth.pointer("/user/id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    // ------------------------------------------------------------ P3：签到与积分

    async fn status(&self, cred: &Credential, http: &dyn Http) -> Result<StatusResult> {
        let (base_summary, balance, lots) = self.quota(cred, http).await?;
        let mut summary = base_summary;
        let mut checked = None;
        let mut next_at = None;

        // 活动接口挂了也要把余额给出去，所以这里不 `?`，失败就保持「状态未知」
        if let Ok(campaigns) = self.campaigns(cred, http).await {
            let now = self.clock.now();
            let benefits = benefits_of(&campaigns, now.timestamp());
            let pending: Vec<&Value> = benefits
                .iter()
                .filter(|c| c.get("claimStatus").and_then(|v| v.as_str()) == Some("CLAIMABLE"))
                .collect();
            let daily = daily_of(&benefits);
            let today = now.date_naive();
            if !pending.is_empty() {
                summary = format!("{summary}；可领活动 {} 项（不等于签到）", pending.len());
            }
            if let Some(daily) = daily {
                let opened = opened_at(daily);
                match daily.get("claimStatus").and_then(|v| v.as_str()) {
                    Some("CLAIMABLE") => checked = Some(false),
                    // 「已领取」只说明这一轮领过。窗口开在今天才算今天已签；
                    // 开在昨天说明今天这轮还没下发，不能显示成「今天已签」。
                    Some("CLAIMED") => {
                        if let Some(opened) = opened {
                            checked = Some(opened.date_naive() == today);
                            next_at = next_window(&benefits);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(StatusResult {
            checked_in: checked,
            summary,
            balance,
            lots: Some(lots),
            next_at,
        })
    }

    async fn credits(&self, cred: &Credential, http: &dyn Http) -> Result<CreditSnapshot> {
        // 只打额度接口，不请求 campaigns，因此不会顺便查签到状态
        let (summary, balance, lots) = self.quota(cred, http).await?;
        Ok(CreditSnapshot {
            summary,
            error: None,
            balance,
            lots: Some(lots),
            quota: None,
        })
    }

    async fn checkin(&self, cred: &Credential, http: &dyn Http) -> Result<ActionResult> {
        let campaigns = self.campaigns(cred, http).await?;
        let now = self.clock.now();
        let benefits = benefits_of(&campaigns, now.timestamp());
        let daily = daily_of(&benefits);
        let targets: Vec<&Value> = daily
            .filter(|c| c.get("claimStatus").and_then(|v| v.as_str()) == Some("CLAIMABLE"))
            .into_iter()
            .collect();

        if targets.is_empty() {
            let claimed =
                daily.filter(|c| c.get("claimStatus").and_then(Value::as_str) == Some("CLAIMED"));
            if let Some(opened) = claimed.and_then(opened_at) {
                if opened.date_naive() < now.date_naive() {
                    let hint = next_window(&benefits)
                        .map(|n| n.format("%H:%M").to_string())
                        .unwrap_or_else(|| "10:00".into());
                    return Ok(ActionResult {
                        ok: true,
                        message: format!("本轮已领取，今日 {hint} 后开放下一轮"),
                        granted: None,
                        validity_days: None,
                        already: true,
                    });
                }
            }
            if claimed.is_some() {
                return Ok(ActionResult {
                    ok: true,
                    message: "今日已领取".into(),
                    granted: None,
                    validity_days: None,
                    already: true,
                });
            }
            return Ok(ActionResult {
                ok: false,
                message: "签到活动未下发（通常 10:00 UTC+8 后开放）".into(),
                granted: None,
                validity_days: None,
                already: false,
            });
        }

        let (mut claimed, mut granted, mut validity_days) = (0usize, 0.0f64, None);
        let mut replay_count = 0;
        let mut failed: Vec<String> = Vec::new();
        for campaign in &targets {
            let id = campaign
                .get("campaignId")
                .map(value_text)
                .unwrap_or_default();
            let url = format!("{}/sash/api/v1/me/campaigns/{id}/claim", self.api_base());
            // 单项网络错误不中断整轮：已领到的别丢信息，未试的接着试
            let resp = match http
                .send(HttpRequest::post(url).headers(&self.headers(cred).await))
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    failed.push(e.message);
                    continue;
                }
            };
            let body = resp.json().unwrap_or(Value::Null);
            let data = body.get("data").filter(|v| v.is_object()).unwrap_or(&body);
            let replayed = data
                .get("replayed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let ok = resp.is_ok()
                && (data.get("status").and_then(|v| v.as_str()) == Some("CLAIMED") || replayed);
            if ok {
                claimed += 1;
                if replayed {
                    replay_count += 1;
                } else {
                    if let Some(amount) =
                        campaign.pointer("/benefit/amount").and_then(|v| v.as_f64())
                    {
                        granted += amount;
                    }
                    let validity = campaign.pointer("/benefit/validity");
                    if validity
                        .and_then(|v| v.get("mode"))
                        .and_then(|v| v.as_str())
                        == Some("RELATIVE_DAYS")
                    {
                        validity_days = validity
                            .and_then(|v| v.get("days"))
                            .and_then(|v| v.as_i64());
                    }
                }
            } else {
                let message = data
                    .get("message")
                    .or_else(|| body.get("message"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("HTTP {}", resp.status));
                failed.push(message);
            }
        }

        if !failed.is_empty() {
            return Ok(ActionResult {
                ok: claimed > 0,
                message: format!("领取 {claimed}/{} 项，失败：{}", targets.len(), failed[0]),
                granted: (granted > 0.0).then_some(granted),
                validity_days,
                already: false,
            });
        }
        let head = if granted > 0.0 {
            format!("领取 {claimed} 项，+{granted} 积分")
        } else {
            format!("领取 {claimed} 项")
        };
        let tail = self
            .credits(cred, http)
            .await
            .map(|c| c.summary)
            .unwrap_or_default();
        Ok(ActionResult {
            ok: true,
            message: if tail.is_empty() {
                head
            } else {
                format!("{head}，{tail}")
            },
            granted: (granted > 0.0).then_some(granted),
            validity_days,
            already: claimed > 0 && replay_count == claimed,
        })
    }
}

/// P3 的接口基址与常量。
const API_CN: &str = "https://openapi.qoder.com.cn";
const API_INTL: &str = "https://openapi.qoder.sh";
const CLIENT_TYPE: &str = "10";
const CLIENT_VERSION: &str = "0.3.4";

impl QoderProvider {
    fn api_base(&self) -> &'static str {
        if self.intl {
            API_INTL
        } else {
            API_CN
        }
    }

    /// async 是给 risk_headers 让路：首次拿设备风控头要跑子进程，不能占 async 线程。
    async fn headers(&self, cred: &Credential) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("User-Agent".to_string(), cred.user_agent("Qoder")),
            (
                "Authorization".to_string(),
                format!("Bearer {}", cred.token),
            ),
        ];
        #[cfg(test)]
        if let Some(extra) = &self.request_headers {
            headers.extend(extra.clone());
            return headers;
        }
        headers.extend(risk_headers(self.intl).await);
        headers
    }

    async fn get_json(&self, cred: &Credential, http: &dyn Http, path: &str) -> Result<Value> {
        let url = format!("{}{path}", self.api_base());
        let resp = http
            .send(HttpRequest::get(url).headers(&self.headers(cred).await))
            .await?;
        if resp.status == 401 {
            return Err(AppError::new(
                ErrorKind::AuthExpired,
                "HTTP 401（登录态可能过期）",
            ));
        }
        if !resp.is_ok() {
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("HTTP {}", resp.status),
            ));
        }
        resp.json()
    }

    async fn campaigns(&self, cred: &Credential, http: &dyn Http) -> Result<Vec<Value>> {
        let doc = self
            .get_json(cred, http, "/sash/api/v1/me/campaigns")
            .await?;
        Ok(doc
            .get("campaigns")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// 套餐额度 + 资源包。返回 (一句话摘要, 余额, 批次)。
    async fn quota(
        &self,
        cred: &Credential,
        http: &dyn Http,
    ) -> Result<(String, Option<f64>, Vec<CreditLot>)> {
        let doc = self.get_json(cred, http, "/api/v2/quota/usage").await?;
        let payload = doc
            .get("qoderUsage")
            .filter(|v| v.is_object())
            .unwrap_or(&doc);

        let mut parts: Vec<String> = Vec::new();
        let mut lots: Vec<CreditLot> = Vec::new();
        let mut balance = 0.0f64;
        let mut counted = false;

        // 企业版会给一个 2100 年的「永不过期」，当成没有到期日
        let plan_expiry = payload
            .get("expiresAt")
            .and_then(dt_of)
            .filter(|d| d.format("%Y").to_string().parse::<i32>().unwrap_or(0) < 2100);

        for (label, key) in [("套餐", "userQuota"), ("资源包", "addOnQuota")] {
            let item = payload.get(key);
            let total = item
                .and_then(|v| v.get("total"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let Some(remaining) = item
                .and_then(|v| v.get("remaining"))
                .and_then(|v| v.as_f64())
            else {
                continue;
            };
            let remaining = remaining.max(0.0);
            if total > 0.0 || remaining > 0.0 {
                parts.push(format!(
                    "{label} {}/{}",
                    trim_f64(remaining),
                    trim_f64(total)
                ));
                balance += remaining;
                counted = true;
            }
            if remaining > 0.0 {
                lots.push(CreditLot {
                    amount: remaining,
                    expires_at: if key == "userQuota" {
                        plan_expiry
                    } else {
                        None
                    },
                    granted_at: None,
                    lot_key: String::new(),
                    label: label.to_string(),
                });
            }
        }

        // 某些组织账号会额外下发独立资源包，各有自己的到期时间
        for item in payload
            .get("dedicatedResourcePackages")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(remaining) = item.get("remaining").and_then(|v| v.as_f64()) else {
                continue;
            };
            if remaining <= 0.0 {
                continue;
            }
            lots.push(CreditLot {
                amount: remaining,
                expires_at: item
                    .get("expiresAt")
                    .or_else(|| item.get("endTime"))
                    .and_then(dt_of),
                granted_at: None,
                lot_key: String::new(),
                label: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("资源包")
                    .to_string(),
            });
            balance += remaining;
            counted = true;
        }

        if payload.get("isQuotaExceeded").and_then(|v| v.as_bool()) == Some(true) {
            parts.push("已超额".to_string());
        }
        let summary = if parts.is_empty() {
            "无可用额度".to_string()
        } else {
            parts.join("，")
        };
        Ok((summary, counted.then_some(balance), lots))
    }
}

/// 每日签到窗口正好是 23 小时 59 分（10:00 → 次日 09:59），
/// 一次性活动不会是这个长度，靠它把两类发币活动区分开。
const DAILY_SPAN: (i64, i64) = (86300, 86400);

/// 只看发币型活动，并限定在当前窗口内。
/// `VIEW_DETAILS` 之类的横幅不是签到，拿它推断「今天已签」必错。
fn benefits_of(campaigns: &[Value], now_secs: i64) -> Vec<Value> {
    campaigns
        .iter()
        .filter(|item| item.get("actionType").and_then(|v| v.as_str()) == Some("CLAIM_BENEFIT"))
        .filter(|item| {
            let start = item.get("startAt").and_then(secs_of);
            let end = item.get("endAt").and_then(secs_of);
            start.map(|s| now_secs >= s).unwrap_or(true)
                && end.map(|e| now_secs < e).unwrap_or(true)
        })
        .cloned()
        .collect()
}

fn daily_of(benefits: &[Value]) -> Option<&Value> {
    benefits.iter().find(|item| {
        let start = item.get("startAt").and_then(secs_of);
        let end = item.get("endAt").and_then(secs_of);
        match (start, end) {
            (Some(s), Some(e)) => (DAILY_SPAN.0..=DAILY_SPAN.1).contains(&(e - s)),
            _ => false,
        }
    })
}

/// 活动窗口的开放时刻。
fn opened_at(campaign: &Value) -> Option<chrono::DateTime<chrono::Local>> {
    campaign.get("startAt").and_then(dt_of)
}

/// 本轮已签后下一次可签的时刻：接口只下发当前窗口，没有排期字段，
/// 只能按同一节奏用 `startAt + 24h` 推算。实际开放时间会漂，界面文案用「约」。
fn next_window(benefits: &[Value]) -> Option<chrono::DateTime<chrono::Local>> {
    let opened = daily_of(benefits).and_then(opened_at)?;
    // 先在 UTC 上加一天再换本地时区，跨夏令时也不会偏一小时
    Some(
        (opened.with_timezone(&chrono::Utc) + chrono::Duration::hours(24))
            .with_timezone(&chrono::Local),
    )
}

/// 接口给的是 unix 秒；个别字段是毫秒，按量级区分。
fn secs_of(value: &Value) -> Option<i64> {
    let n = value.as_f64()?;
    if n > 1e11 {
        Some((n / 1000.0) as i64)
    } else {
        Some(n as i64)
    }
}

fn dt_of(value: &Value) -> Option<chrono::DateTime<chrono::Local>> {
    let secs = if value.is_string() {
        let text = value.as_str()?.trim().replace('Z', "+00:00");
        chrono::DateTime::parse_from_rfc3339(&text)
            .ok()?
            .timestamp()
    } else {
        secs_of(value)?
    };
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.with_timezone(&chrono::Local))
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 设备风控头。签到活动接口从 2026-09-26 起要求携带 `Cosy-Machine*`，
/// 否则服务端不下发每日活动。设备标识不伪造，直接跑客户端自带的
/// `resources/umid/runtime-info.exe` 取；拿不到就退回最小可用头。
/// async：runtime-info.exe 是同步轮询等退出（最长 12s），必须进阻塞池，
/// 挂在 async 线程上会让并发刷积分把 worker 全部占死。
async fn risk_headers(intl: bool) -> Vec<(String, String)> {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    // type_complexity：一次性缓存的三元组，type alias 只在这里用一次
    #[allow(clippy::type_complexity)]
    static CACHE: OnceLock<Mutex<Option<(bool, Instant, Vec<(String, String)>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((flag, at, headers)) = guard.as_ref() {
            if *flag == intl && at.elapsed() < Duration::from_secs(1800) {
                return headers.clone();
            }
        }
    }

    let mut headers = vec![("Cosy-ClientType".to_string(), CLIENT_TYPE.to_string())];
    let profile = default_profile(intl);
    let root = install_dir(intl);

    if let Some(root) = root {
        let exe = root.join("resources").join("umid").join("runtime-info.exe");
        let data: Value = match crate::app::runtime::blocking(move || {
            process::run_capture(&exe, Duration::from_secs(12))
        })
        .await
        {
            // 内层 run_capture 失败、外层任务崩溃，都按「拿不到」退回最小头
            Ok(Ok(out)) => serde_json::from_str(out.trim()).unwrap_or(Value::Null),
            Ok(Err(_)) | Err(_) => Value::Null,
        };

        headers.push(("Cosy-MachineOS".into(), "x86_64_windows".into()));
        if let Ok(host) = std::env::var("COMPUTERNAME") {
            headers.push(("Cosy-MachineHostname".into(), host));
        }
        let version = std::fs::read_to_string(root.join("resources").join("build-manifest.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| {
                v.get("productVersion")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| CLIENT_VERSION.to_string());
        headers.push(("Cosy-Version".into(), version));

        if let Some(dir) = profile {
            if let Ok(machine_id) = std::fs::read_to_string(dir.join("auth.machine-id")) {
                let machine_id = machine_id.trim();
                if !machine_id.is_empty() {
                    headers.push(("Cosy-MachineId".into(), machine_id.to_string()));
                }
            }
        }
        for (source, target) in [
            ("machineToken", "Cosy-MachineToken"),
            ("machineCode", "Cosy-MachineCode"),
            ("machineType", "Cosy-MachineType"),
        ] {
            if let Some(value) = data.get(source).and_then(|v| v.as_str()) {
                if !value.is_empty() {
                    headers.push((target.to_string(), value.to_string()));
                }
            }
        }
    }

    if let Ok(mut guard) = cache.lock() {
        *guard = Some((intl, Instant::now(), headers.clone()));
    }
    headers
}

fn install_dir(intl: bool) -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("Programs");
    let names: &[&str] = if intl {
        &["Qoder"]
    } else {
        &["Qoder CN", "Qoder"]
    };
    names.iter().map(|name| base.join(name)).find(|root| {
        root.join("resources")
            .join("umid")
            .join("runtime-info.exe")
            .exists()
    })
}

fn tail(text: &str, n: usize) -> String {
    text.chars()
        .rev()
        .take(n)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn default_profile(intl: bool) -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    let name = if intl {
        "com.qoder.app.stable"
    } else {
        "com.qodercn.app.stable"
    };
    Some(PathBuf::from(appdata).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn checkin_fixture(intl: bool) -> (QoderProvider, crate::app::net::MockHttp, i64) {
        let now = crate::app::clock::local(2026, 10, 30, 9, 0);
        let mut provider = QoderProvider::new(intl);
        provider.clock = Arc::new(crate::app::clock::FakeClock::at(&now.to_rfc3339()));
        provider.request_headers = Some(Vec::new());
        let http = crate::app::net::MockHttp::new();
        http.on(
            &format!("{}/api/v2/quota/usage", provider.api_base()),
            200,
            r#"{"userQuota":{"total":100,"remaining":50}}"#,
        );
        (provider, http, now.timestamp())
    }

    fn campaign(id: &str, start: i64, span: i64, status: &str) -> Value {
        json!({
            "campaignId": id, "actionType": "CLAIM_BENEFIT",
            "startAt": start, "endAt": start + span, "claimStatus": status,
            "benefit": {"amount": 10, "validity": {"mode": "RELATIVE_DAYS", "days": 7}}
        })
    }

    fn serve_campaigns(
        provider: &QoderProvider,
        http: &crate::app::net::MockHttp,
        items: Vec<Value>,
    ) {
        http.on(
            &format!("{}/sash/api/v1/me/campaigns", provider.api_base()),
            200,
            &json!({"campaigns": items}).to_string(),
        );
    }

    #[tokio::test]
    async fn bonus_claim_does_not_count_as_daily_checkin() {
        for intl in [false, true] {
            let (provider, http, now) = checkin_fixture(intl);
            serve_campaigns(
                &provider,
                &http,
                vec![campaign("bonus", now - 3600, 7 * 86400, "CLAIMABLE")],
            );
            let bonus_url = format!(
                "{}/sash/api/v1/me/campaigns/bonus/claim",
                provider.api_base()
            );
            http.on(&bonus_url, 200, r#"{"status":"CLAIMED"}"#);
            let result = provider.checkin(&sample_cred(), &http).await.unwrap();
            assert!(!result.ok, "没有每日签到活动，不能用赠送领取冒充签到成功");
            assert!(!result.already);
            assert_eq!(http.call_count(&bonus_url), 0);
        }
    }

    #[tokio::test]
    async fn bonus_does_not_hide_next_daily_window() {
        for intl in [false, true] {
            let (provider, http, now) = checkin_fixture(intl);
            let start = now - 23 * 3600;
            serve_campaigns(
                &provider,
                &http,
                vec![
                    campaign("daily", start, 86340, "CLAIMED"),
                    campaign("bonus", now - 3600, 7 * 86400, "CLAIMABLE"),
                ],
            );
            let state = provider.status(&sample_cred(), &http).await.unwrap();
            assert_eq!(state.checked_in, Some(false));
            assert_eq!(state.next_at.map(|at| at.timestamp()), Some(start + 86400));
            let result = provider.checkin(&sample_cred(), &http).await.unwrap();
            assert!(result.already);
            assert_eq!(result.granted, None);
        }
    }

    #[tokio::test]
    async fn pending_bonus_does_not_override_completed_daily_checkin() {
        let (provider, http, now) = checkin_fixture(true);
        serve_campaigns(
            &provider,
            &http,
            vec![
                campaign("daily", now - 3600, 86340, "CLAIMED"),
                campaign("bonus", now - 3600, 7 * 86400, "CLAIMABLE"),
            ],
        );
        let state = provider.status(&sample_cred(), &http).await.unwrap();
        assert_eq!(state.checked_in, Some(true));
    }

    #[tokio::test]
    async fn failed_daily_is_not_rescued_by_successful_bonus() {
        let (provider, http, now) = checkin_fixture(true);
        serve_campaigns(
            &provider,
            &http,
            vec![
                campaign("daily", now - 3600, 86340, "CLAIMABLE"),
                campaign("bonus", now - 3600, 7 * 86400, "CLAIMABLE"),
            ],
        );
        let daily_url = format!(
            "{}/sash/api/v1/me/campaigns/daily/claim",
            provider.api_base()
        );
        let bonus_url = format!(
            "{}/sash/api/v1/me/campaigns/bonus/claim",
            provider.api_base()
        );
        http.on(&daily_url, 500, r#"{"message":"daily failed"}"#);
        http.on(&bonus_url, 200, r#"{"status":"CLAIMED"}"#);
        let result = provider.checkin(&sample_cred(), &http).await.unwrap();
        assert!(!result.ok);
        assert_eq!(result.granted, None);
        assert_eq!(http.call_count(&bonus_url), 0);
    }

    #[tokio::test]
    async fn missing_or_unknown_daily_state_is_not_evidence_of_unchecked() {
        for status in [None, Some("UNKNOWN")] {
            let (provider, http, now) = checkin_fixture(true);
            let mut campaigns = vec![campaign("bonus", now - 3600, 7 * 86400, "CLAIMED")];
            if let Some(status) = status {
                campaigns.push(campaign("daily", now - 3600, 86340, status));
            }
            serve_campaigns(&provider, &http, campaigns);
            let state = provider.status(&sample_cred(), &http).await.unwrap();
            assert_eq!(
                state.checked_in, None,
                "每日活动缺失或状态未知时，不能否定已有签到记录"
            );
        }
    }

    #[tokio::test]
    async fn daily_claim_excludes_bonus_amount_and_replays() {
        for replayed in [false, true] {
            let (provider, http, now) = checkin_fixture(true);
            serve_campaigns(
                &provider,
                &http,
                vec![
                    campaign("daily", now - 3600, 86340, "CLAIMABLE"),
                    campaign("bonus", now - 3600, 7 * 86400, "CLAIMABLE"),
                ],
            );
            let daily_url = format!(
                "{}/sash/api/v1/me/campaigns/daily/claim",
                provider.api_base()
            );
            let bonus_url = format!(
                "{}/sash/api/v1/me/campaigns/bonus/claim",
                provider.api_base()
            );
            http.on(
                &daily_url,
                200,
                &json!({"status": "CLAIMED", "replayed": replayed}).to_string(),
            );
            http.on(&bonus_url, 200, r#"{"status":"CLAIMED"}"#);
            let result = provider.checkin(&sample_cred(), &http).await.unwrap();
            assert!(result.ok);
            assert_eq!(result.already, replayed);
            assert_eq!(result.granted, if replayed { None } else { Some(10.0) });
            assert_eq!(http.call_count(&daily_url), 1);
            assert_eq!(http.call_count(&bonus_url), 0);
        }
    }

    fn sample_cred() -> Credential {
        let mut cred = Credential {
            token: "tk".into(),
            refresh_token: "rt".into(),
            expires_at: "2026-11-01T00:00:00Z".into(),
            ..Default::default()
        };
        cred.put("email", json!("a@b.c"));
        cred
    }

    #[test]
    fn build_auth_writes_token_and_identity() {
        let auth = build_auth(&sample_cred(), "u1", "某人", None);
        assert_eq!(auth.pointer("/user/id").unwrap(), "u1");
        assert_eq!(auth.pointer("/user/name").unwrap(), "某人");
        assert_eq!(auth.pointer("/token").unwrap(), "tk");
        assert_eq!(auth.pointer("/refreshToken").unwrap(), "rt");
        assert_eq!(auth.pointer("/expiresAt").unwrap(), "2026-11-01T00:00:00Z");
    }

    #[test]
    fn other_users_phone_and_avatar_are_not_carried_over() {
        let current = json!({"user": {"id": "u2", "phone": "13800000000", "avatarUrl": "x"}});
        let auth = build_auth(&sample_cred(), "u1", "某人", Some(&current));
        assert!(
            auth.pointer("/user/phone").unwrap().is_null(),
            "不能把别人的手机号写给这个账号"
        );
        assert!(auth.pointer("/user/avatarUrl").unwrap().is_null());
    }

    #[test]
    fn same_users_extra_fields_are_kept() {
        let current = json!({"user": {"id": "u1", "phone": "13800000000"}, "schemaVersion": 3,
                             "authenticationContext": {"x": 1}});
        let auth = build_auth(&sample_cred(), "u1", "某人", Some(&current));
        assert_eq!(auth.pointer("/user/phone").unwrap(), "13800000000");
        assert_eq!(auth.pointer("/schemaVersion").unwrap(), 3);
        assert_eq!(auth.pointer("/authenticationContext/x").unwrap(), 1);
    }

    #[test]
    fn missing_expiry_does_not_write_empty_string() {
        let cred = Credential {
            token: "t".into(),
            refresh_token: "r".into(),
            ..Default::default()
        };
        let auth = build_auth(&cred, "u1", "n", None);
        let expires = auth.pointer("/expiresAt").unwrap().as_str().unwrap();
        assert!(!expires.is_empty(), "空串会让客户端的时间解析失败");
    }

    #[test]
    fn seal_then_open_roundtrip() {
        let key = *Key::<Aes256Gcm>::from_slice(&[9u8; 32]);
        let sealed = QoderProvider::seal(&key, br#"{"token":"x"}"#).unwrap();
        assert_eq!(&sealed[..3], b"v10");
        assert_eq!(
            secret::chromium_open(&key, &sealed).unwrap(),
            br#"{"token":"x"}"#
        );
    }

    /// 造一个「客户端已经登录过 old」的 profile：Local State（DPAPI 包着 32 字节密钥）
    /// + auth.v1.dat（v10 信封）。这样整条切换路径都能在临时目录里跑。
    fn fake_profile(dir: &Path) -> Key<Aes256Gcm> {
        let key_bytes = [7u8; 32];
        let wrapped = secret::protect(&key_bytes).unwrap();
        let mut encrypted_key = b"DPAPI".to_vec();
        encrypted_key.extend(wrapped);
        let state = json!({
            "os_crypt": {
                "encrypted_key": base64::engine::general_purpose::STANDARD.encode(&encrypted_key)
            }
        });
        std::fs::write(dir.join("Local State"), serde_json::to_vec(&state).unwrap()).unwrap();
        let key = *Key::<Aes256Gcm>::from_slice(&key_bytes);
        let sealed = QoderProvider::seal(
            &key,
            &serde_json::to_vec(&json!({
                "schemaVersion": 2,
                "token": "old-token",
                "user": {"id": "old", "name": "旧账号", "phone": "13800000000"}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("auth.v1.dat"), sealed).unwrap();
        key
    }

    #[test]
    fn switch_rewrites_login_state_and_backs_up() {
        let dir = tempfile::TempDir::new().unwrap();
        let key = fake_profile(dir.path());
        let provider = QoderProvider::at(false, dir.path().to_path_buf()).assume_alive(false);
        let mut cred = Credential {
            token: "new-token".into(),
            refresh_token: "new-refresh".into(),
            expires_at: "2026-12-01T00:00:00Z".into(),
            ..Default::default()
        };
        cred.put("email", json!("new@b.c"));

        switch_to(&provider, "u2", "新账号", &cred).unwrap();

        let raw = std::fs::read(dir.path().join("auth.v1.dat")).unwrap();
        assert_eq!(&raw[..3], b"v10");
        let plain: Value =
            serde_json::from_slice(&secret::chromium_open(&key, &raw).unwrap()).unwrap();
        assert_eq!(plain.pointer("/user/id").unwrap(), "u2");
        assert_eq!(plain.pointer("/token").unwrap(), "new-token");
        assert_eq!(plain.pointer("/refreshToken").unwrap(), "new-refresh");
        // 模板字段沿用，但别人的手机号不能跟过来
        assert_eq!(plain.pointer("/schemaVersion").unwrap(), 2);
        assert!(plain.pointer("/user/phone").unwrap().is_null());
        assert_eq!(provider.current_id().as_deref(), Some("u2"));
    }

    #[test]
    fn switch_refuses_the_account_already_logged_in() {
        let dir = tempfile::TempDir::new().unwrap();
        fake_profile(dir.path());
        let provider = QoderProvider::at(false, dir.path().to_path_buf()).assume_alive(false);
        let cred = sample_cred();
        let err = switch_to(&provider, "old", "旧账号", &cred).unwrap_err();
        assert!(err.message.contains("不需要切换"));
    }

    #[test]
    fn switch_refuses_when_client_running() {
        // 客户端在跑：必须在写任何东西之前就退，并给出 client_running
        let dir = tempfile::TempDir::new().unwrap();
        fake_profile(dir.path());
        let provider = QoderProvider::at(false, dir.path().to_path_buf()).assume_alive(true);

        let err = switch_to(&provider, "u2", "新账号", &sample_cred()).unwrap_err();
        assert_eq!(err.kind, crate::app::error::ErrorKind::ClientRunning);
    }

    #[test]
    fn switch_refuses_when_login_files_missing() {
        // 客户端没开，但目录里没有登录态文件：报 io，不是 client_running
        let dir = tempfile::TempDir::new().unwrap();
        let provider = QoderProvider::at(false, dir.path().to_path_buf()).assume_alive(false);
        let err = switch_to(&provider, "u1", "n", &sample_cred()).unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Io));
    }

    #[test]
    fn missing_profile_returns_zero_not_error() {
        let provider = QoderProvider::new(false);
        if provider.profile_dir().map(|p| p.exists()).unwrap_or(false) {
            return; // 本机装了，跳过这条
        }
        assert_eq!(provider.scan(&mut |_| Ok(())).unwrap(), 0);
    }

    /// 真实机验证：本机装有 Qoder 才跑
    /// `cargo test --lib qoder -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scans_real_qoder_login_state() {
        for intl in [false, true] {
            let provider = QoderProvider::new(intl);
            let mut found = Vec::new();
            let count = provider
                .scan(&mut |acc| {
                    found.push((
                        acc.identity.name.clone(),
                        acc.identity.iid.clone(),
                        acc.secret.len(),
                    ));
                    Ok(())
                })
                .expect("扫描不应失败");
            println!("{}: {count} 个账号", provider.label());
            for (name, iid, size) in &found {
                println!(
                    "   {name} · {} · 凭据 {size} 字节",
                    &iid[..iid.len().min(24)]
                );
            }
        }
    }
}
