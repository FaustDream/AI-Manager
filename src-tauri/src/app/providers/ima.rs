//! ima（腾讯 ima.copilot）：读本机客户端 Cookie，每天签到领「算力」。
//!
//! 协议来自可运行的开源签到脚本（实测）：
//!   - access token 只有 2 小时，每次查询都先用 refresh token 换一次；
//!   - 活动面板与签到都要 `x-ima-bkn`（djb2 变体，int32 溢出回绕、最终 & 0x7FFFFFFF）；
//!   - 签到接口连续被拒后回 HTTP 403 + 空 body，必须按错误接住，不能 panic。
//!
//! 凭据边界：refresh token / access token 只在本模块内存里走一次请求，
//! 不写日志、不进错误文案、不随返回值带出。

use crate::app::accounts::credential::Credential;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::app::providers::{Provider, ScannedAccount, Sink};
use crate::types::{trim_f64, ActionResult, CreditSnapshot, StatusResult};
use aes_gcm::{Aes256Gcm, Key};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const REFRESH_URL: &str = "https://ima.qq.com/cgi-bin/auth_login/refresh";
const ACTIVITY_URL: &str = "https://ima.qq.com/cgi-bin/daily_login_activity/get_activity_info";
/// 「我的余额」：活动赠送 + 付费余额。活动面板的 `total_reward_points` 只是
/// 签到活动累计发放的奖励，不等于当前可用算力（客户端「我的余额」也不读它）。
const BALANCE_URL: &str = "https://ima.qq.com/cgi-bin/token_manager/get_balance";
const CHECKIN_URL: &str = "https://ima.qq.com/cgi-bin/daily_login_activity/check_in";
/// 客户端版本标记是协议的一部分：服务端按它区分浏览器/客户端请求。
const CLIENT_VERSION: &str = "5.17.2";
const REQUIRED_COOKIES: [&str; 5] = [
    "IMA-UID",
    "UID-TYPE",
    "IMA-GUID",
    "IMA-TOKEN",
    "IMA-REFRESH-TOKEN",
];
const TODAY: &str = "今日";
const FULL_WEEK: &str = "满签奖励";

pub struct ImaProvider {
    /// `%LOCALAPPDATA%\ima.copilot\User Data`；测试可注入临时目录。
    root: Option<PathBuf>,
}

impl ImaProvider {
    pub fn new() -> Self {
        Self {
            root: std::env::var("LOCALAPPDATA")
                .ok()
                .map(|base| PathBuf::from(base).join("ima.copilot").join("User Data")),
        }
    }

    /// 只给测试用：指向一个构造出来的客户端数据目录。
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    /// 三处 Cookie 库都要扫：新版把主库放在 Network/，旧版在 Default/，
    /// 扩展（客户端本体）用 Extension Cookies。
    fn cookie_files(&self) -> Vec<PathBuf> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        vec![
            root.join("Default").join("Network").join("Cookies"),
            root.join("Default").join("Cookies"),
            root.join("Default").join("Extension Cookies"),
        ]
        .into_iter()
        .filter(|path| path.is_file())
        .collect()
    }

    /// 读本机登录态。没装客户端返回 `None`；库被占用/加密格式不支持才报错。
    fn cookies(&self) -> Result<Option<CookieSet>> {
        let files = self.cookie_files();
        if files.is_empty() {
            return Ok(None);
        }
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorKind::Io, "没有找到 ima 客户端数据目录"))?;
        let key = crate::app::secret::chromium_key(&root.join("Local State")).map_err(|e| {
            AppError::new(
                ErrorKind::Io,
                format!("读取 ima 客户端加密密钥失败：{}", e.message),
            )
            .with_detail(e.evidence())
        })?;

        let mut found: HashMap<String, String> = HashMap::new();
        let mut source = String::new();
        let mut unreadable = 0usize;
        let mut parse_error: Option<AppError> = None;
        for path in files {
            let Some(temp) = copy_to_temp(&path).ok().flatten() else {
                // 客户端运行时会把主库锁成独占；这不是「没装」，必须让用户看到原因
                unreadable += 1;
                continue;
            };
            let parsed = read_cookie_pairs(&temp, &key);
            let _ = std::fs::remove_file(&temp);
            match parsed {
                Ok(pairs) => {
                    for (name, value) in pairs {
                        if found.contains_key(&name) {
                            continue;
                        }
                        if source.is_empty() {
                            source = cookie_source(&path);
                        }
                        found.insert(name, value);
                    }
                }
                // v20 是应用绑定加密：明确说「不支持」，不要笼统报成「库被占用」
                Err(e) if e.kind == ErrorKind::Unsupported => return Err(e),
                Err(e) => parse_error = Some(e),
            }
            if found.len() == REQUIRED_COOKIES.len() {
                break;
            }
        }

        if found.len() == REQUIRED_COOKIES.len() {
            return Ok(Some(CookieSet::from_pairs(found, source)?));
        }
        if let Some(error) = parse_error {
            return Err(error);
        }
        if unreadable > 0 {
            return Err(AppError::new(
                ErrorKind::Io,
                "ima 客户端正在运行并占用了 Cookie 库，请完全退出 ima 客户端后重试",
            ));
        }
        if !found.is_empty() {
            // 库能读、也有关键 Cookie 的痕迹，但凑不齐登录态：通常是客户端退出了登录
            return Err(AppError::new(
                ErrorKind::AuthExpired,
                "ima 客户端里没有完整的登录态，请先在客户端登录后重新扫描",
            ));
        }
        Ok(None)
    }

    /// 用 refresh token 换 access token。code!=0 或没给 token 都算登录失效。
    async fn refresh_access_token(&self, cred: &Credential, http: &dyn Http) -> Result<String> {
        let saved = SavedCredential::from(cred)?;
        let body = json!({
            "userId": json_uid(&saved.ima_uid),
            "refreshToken": saved.refresh_token,
            "tokenType": -1,
        });
        let req = HttpRequest::post(REFRESH_URL)
            .header("from_browser_ima", "1")
            .header("extension_version", CLIENT_VERSION)
            .header(
                "x-ima-cookie",
                format!("IMA-GUID={}; PLATFORM=H5", saved.ima_guid),
            )
            .json_body(&body);
        let resp = http.send(req).await?;
        if !resp.is_ok() {
            return Err(http_error("刷新 ima 访问令牌", resp.status));
        }
        let value = resp.json()?;
        let code = value.get("code").and_then(as_i64).unwrap_or(-1);
        let token = value
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if code != 0 || token.is_empty() {
            return Err(login_expired());
        }
        Ok(token)
    }

    /// 账号快照：签到状态 + **当前可用算力**。
    ///
    /// 两个接口共用一次 refresh 换来的 token：活动面板只回答「今天签没签」，
    /// 余额必须单独查 `token_manager/get_balance`，不能拿活动累计当余额。
    async fn snapshot(
        &self,
        cred: &Credential,
        http: &dyn Http,
    ) -> Result<(Activity, Option<f64>)> {
        let saved = SavedCredential::from(cred)?;
        let token = self.refresh_access_token(cred, http).await?;
        let activity = self.activity_with_token(&saved, http, &token).await?;
        let balance = self.balance_with_token(&saved, http, &token).await?;
        Ok((activity, balance))
    }

    /// 用已有的 access token 查活动（签到前后回查共用同一个 token）。
    async fn activity_with_token(
        &self,
        saved: &SavedCredential,
        http: &dyn Http,
        token: &str,
    ) -> Result<Activity> {
        let req = HttpRequest::post(ACTIVITY_URL)
            .header("from_browser_ima", "1")
            .header("extension_version", CLIENT_VERSION)
            .header("x-ima-cookie", saved.cookie_header(token))
            .header("x-ima-bkn", bkn(token))
            .json_body(&json!({}));
        let resp = http.send(req).await?;
        if !resp.is_ok() {
            return Err(http_error("查询 ima 签到活动", resp.status));
        }
        let value = resp.json()?;
        let code = value.get("code").and_then(as_i64).unwrap_or(-1);
        if code != 0 {
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("ima 签到活动接口返回异常（code {code}）"),
            ));
        }
        Ok(Activity::parse(&value))
    }

    /// 当前可用算力。返回 `None` 表示接口成功但没给数值，不代表 0。
    async fn balance_with_token(
        &self,
        saved: &SavedCredential,
        http: &dyn Http,
        token: &str,
    ) -> Result<Option<f64>> {
        let req = HttpRequest::post(BALANCE_URL)
            .header("from_browser_ima", "1")
            .header("extension_version", CLIENT_VERSION)
            .header("x-ima-cookie", saved.cookie_header(token))
            .header("x-ima-bkn", bkn(token))
            .json_body(&json!({}));
        let resp = http.send(req).await?;
        if !resp.is_ok() {
            return Err(http_error("查询 ima 算力余额", resp.status));
        }
        let value = resp.json()?;
        let code = value.get("code").and_then(as_i64).unwrap_or(-1);
        if code != 0 {
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("ima 算力余额接口返回异常（code {code}）"),
            ));
        }
        Ok(parse_balance(&value))
    }

    /// 发一次签到。`kind=1` 是日签，`kind=2` 是周日满签奖励。
    async fn post_checkin(
        &self,
        saved: &SavedCredential,
        http: &dyn Http,
        token: &str,
        kind: i64,
    ) -> Result<Value> {
        let req = HttpRequest::post(CHECKIN_URL)
            .header("from_browser_ima", "1")
            .header("extension_version", CLIENT_VERSION)
            .header("x-ima-cookie", saved.cookie_header(token))
            .header("x-ima-bkn", bkn(token))
            .json_body(&json!({ "type": kind }));
        let resp = http.send(req).await?;
        // 403 是连续被拒后的风控回包，body 为空；这里给可读错误，绝不 panic
        if resp.status == 403 {
            return Err(AppError::new(
                ErrorKind::Provider,
                "ima 拒绝了签到请求（HTTP 403），可能触发风控，请稍后再试",
            ));
        }
        if !resp.is_ok() {
            return Err(http_error("提交 ima 签到", resp.status));
        }
        resp.json()
    }
}

impl Default for ImaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for ImaProvider {
    fn platform(&self) -> &'static str {
        "ima"
    }

    fn label(&self) -> &'static str {
        "ima"
    }

    fn scan(&self, sink: &mut Sink<'_>) -> Result<usize> {
        let Some(jar) = self.cookies()? else {
            return Ok(0);
        };
        let identity = crate::app::accounts::AccountIdentity {
            iid: format!("ima:{}", jar.ima_uid),
            platform: self.platform().to_string(),
            platform_label: self.label().to_string(),
            account_id: jar.ima_uid.clone(),
            // Cookie 里没有昵称；只显示平台名，绝不把 UID 当昵称（契约 17）
            name: "ima 账号".into(),
            expires_at: None,
            source: jar.source,
            requires_checkin: true,
            has_secret: true,
        };
        let mut cred = Credential {
            refresh_token: jar.refresh_token,
            ..Default::default()
        };
        cred.put("ima_uid", json!(jar.ima_uid));
        cred.put("uid_type", json!(jar.uid_type));
        cred.put("ima_guid", json!(jar.ima_guid));
        sink(ScannedAccount::new(identity, &cred)?)?;
        Ok(1)
    }

    fn current_id(&self) -> Option<String> {
        self.cookies().ok().flatten().map(|jar| jar.ima_uid)
    }

    fn credits_min_interval_minutes(&self) -> u64 {
        0
    }

    async fn status(&self, cred: &Credential, http: &dyn Http) -> Result<StatusResult> {
        let (activity, balance) = self.snapshot(cred, http).await?;
        Ok(StatusResult {
            checked_in: activity.checked_in,
            summary: activity.summary(balance),
            balance,
            lots: None,
            // 接口不下发「下次可签时刻」，不按本地时钟编一个
            next_at: None,
        })
    }

    async fn credits(&self, cred: &Credential, http: &dyn Http) -> Result<CreditSnapshot> {
        let (activity, balance) = self.snapshot(cred, http).await?;
        Ok(CreditSnapshot {
            summary: activity.summary(balance),
            error: None,
            balance,
            // 平台只回当前余额，没有发放批次与到期日
            lots: None,
            quota: None,
        })
    }

    async fn checkin(&self, cred: &Credential, http: &dyn Http) -> Result<ActionResult> {
        let saved = SavedCredential::from(cred)?;
        // 一次刷新换来的 token 贯穿「查活动 → 日签 → 回查 → 满签」，
        // 不重复打 refresh 接口（重复换 token 更容易触发风控）。
        let token = self.refresh_access_token(cred, http).await?;
        let before = self.activity_with_token(&saved, http, &token).await?;
        let today = before.today_status;

        if today == Some(2) {
            let balance = self.balance_with_token(&saved, http, &token).await.ok().flatten();
            return Ok(ActionResult {
                ok: true,
                message: format!("今日已签到，{}", balance_text(balance)),
                granted: None,
                validity_days: None,
                already: true,
            });
        }
        if today != Some(3) {
            return Ok(ActionResult {
                ok: false,
                message: "当前没有可领取的签到奖励（活动未开放或今日已过期）".into(),
                granted: None,
                validity_days: None,
                already: false,
            });
        }

        let value = self.post_checkin(&saved, http, &token, 1).await?;
        let code = value.get("code").and_then(as_i64).unwrap_or(-1);
        if code == 1002 {
            let balance = self.balance_with_token(&saved, http, &token).await.ok().flatten();
            return Ok(ActionResult {
                ok: true,
                message: format!("今日已签到，{}", balance_text(balance)),
                granted: None,
                validity_days: None,
                already: true,
            });
        }
        if code == 1001 {
            return Ok(ActionResult {
                ok: false,
                message: "ima 本周还没签满，今天的签到未能领取".into(),
                granted: None,
                validity_days: None,
                already: false,
            });
        }
        if code != 0 {
            return Ok(ActionResult {
                ok: false,
                message: format!("ima 签到未成功（code {code}）"),
                granted: None,
                validity_days: None,
                already: false,
            });
        }

        let granted = value
            .get("reward_received")
            .and_then(as_f64)
            .filter(|v| *v > 0.0);

        // 日签后再回查一次：算力余额与已签到天数都以平台最新值为准。
        let latest = match self.activity_with_token(&saved, http, &token).await {
            Ok(activity) => Some(activity),
            Err(e) => {
                crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "checkin",
                    format!("ima 签到后回查活动失败：{}", e.message),
                );
                None
            }
        };

        // 满签奖励由平台在周日且本周签满后才解锁（面板 status=3），
        // 所以这里以平台状态为准，不用本地星期几再判一次。
        let mut full_week = false;
        if latest
            .as_ref()
            .and_then(|activity| activity.full_week_status)
            == Some(3)
        {
            match self.post_checkin(&saved, http, &token, 2).await {
                Ok(value) => {
                    let code = value.get("code").and_then(as_i64).unwrap_or(-1);
                    if code == 0 {
                        full_week = true;
                    } else {
                        crate::app::log::log(
                            crate::app::log::Level::Warn,
                            "checkin",
                            format!("ima 满签奖励未领取（code {code}），不影响日签结果"),
                        );
                    }
                }
                Err(e) => crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "checkin",
                    format!("ima 满签奖励请求失败：{}，不影响日签结果", e.message),
                ),
            }
        }

        // 余额放在满签奖励之后再查：周日两笔一起入账时，这里拿到的是最终值。
        // 余额查不到不影响签到结果，只把文案退成「暂未返回」。
        let balance = match self.balance_with_token(&saved, http, &token).await {
            Ok(balance) => balance,
            Err(e) => {
                crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "checkin",
                    format!("ima 签到后查询余额失败：{}", e.message),
                );
                None
            }
        };
        let mut message = format!("签到成功，{}", balance_text(balance));
        if full_week {
            message.push_str("，满签奖励已领取");
        }
        Ok(ActionResult {
            ok: true,
            message,
            granted,
            validity_days: None,
            already: false,
        })
    }
}

// ------------------------------------------------------------------ 协议数据结构

struct SavedCredential {
    ima_uid: String,
    uid_type: String,
    ima_guid: String,
    refresh_token: String,
}

impl SavedCredential {
    fn from(cred: &Credential) -> Result<Self> {
        let missing = || {
            AppError::new(
                ErrorKind::AuthExpired,
                "ima 凭据不完整，请打开 ima 客户端确认登录后重新扫描",
            )
        };
        let ima_uid = cred
            .meta_str("ima_uid")
            .filter(|s| !s.is_empty())
            .ok_or_else(missing)?;
        let uid_type = cred
            .meta_str("uid_type")
            .filter(|s| !s.is_empty())
            .ok_or_else(missing)?;
        let ima_guid = cred
            .meta_str("ima_guid")
            .filter(|s| !s.is_empty())
            .ok_or_else(missing)?;
        let refresh_token = cred.refresh().trim();
        if refresh_token.is_empty() {
            return Err(missing());
        }
        Ok(Self {
            ima_uid: ima_uid.to_string(),
            uid_type: uid_type.to_string(),
            ima_guid: ima_guid.to_string(),
            refresh_token: refresh_token.to_string(),
        })
    }

    fn cookie_header(&self, token: &str) -> String {
        format!(
            "IMA-UID={}; IMA-TOKEN={}; IMA-REFRESH-TOKEN={}; UID-TYPE={}; IMA-GUID={}; PLATFORM=H5",
            self.ima_uid, token, self.refresh_token, self.uid_type, self.ima_guid
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Activity {
    today_status: Option<i64>,
    full_week_status: Option<i64>,
    checked_in: Option<bool>,
    checkin_days: Option<i64>,
}

impl Activity {
    fn parse(value: &Value) -> Self {
        let mut today_status = None;
        let mut full_week_status = None;
        if let Some(infos) = value.get("infos").and_then(Value::as_array) {
            for info in infos {
                let status = info.get("status").and_then(as_i64);
                match info.get("top").and_then(Value::as_str) {
                    Some(TODAY) => today_status = status,
                    Some(FULL_WEEK) => full_week_status = status,
                    _ => {}
                }
            }
        }
        Self {
            today_status,
            full_week_status,
            checked_in: today_status.map(|status| status == 2),
            checkin_days: value.get("checkin_days").and_then(as_i64),
        }
    }

    /// 余额来自 `get_balance`，活动面板不提供可用算力。
    fn summary(&self, balance: Option<f64>) -> String {
        let mut parts = Vec::new();
        if let Some(balance) = balance {
            parts.push(format!("算力 {}", trim_f64(balance)));
        }
        if let Some(days) = self.checkin_days {
            parts.push(format!("已签到 {days} 天"));
        }
        if parts.is_empty() {
            "ima 暂无算力数据".into()
        } else {
            parts.join(" · ")
        }
    }
}

struct CookieSet {
    ima_uid: String,
    uid_type: String,
    ima_guid: String,
    refresh_token: String,
    source: String,
}

impl CookieSet {
    fn from_pairs(
        pairs: impl IntoIterator<Item = (String, String)>,
        source: String,
    ) -> Result<Self> {
        let map: HashMap<String, String> = pairs.into_iter().collect();
        let get = |name: &str| {
            map.get(name)
                .filter(|value| !value.is_empty())
                .cloned()
                .ok_or_else(|| {
                    AppError::new(
                        ErrorKind::AuthExpired,
                        "ima 客户端里没有完整的登录态，请先登录后重新扫描",
                    )
                })
        };
        // IMA-TOKEN 只用于确认登录态完整；access token 2 小时就过期，
        // 每次请求都重新换，不把它存进账号库。
        let _access_token = get("IMA-TOKEN")?;
        Ok(Self {
            ima_uid: get("IMA-UID")?,
            uid_type: get("UID-TYPE")?,
            ima_guid: get("IMA-GUID")?,
            refresh_token: get("IMA-REFRESH-TOKEN")?,
            source,
        })
    }
}

// ------------------------------------------------------------------ Cookie 库

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 复制到 tempfile 再读：客户端运行时持有 Cookie 库的独占句柄，直接开只会报占用。
/// 返回 `Ok(None)` 表示源文件已经消失（例如客户端刚退出清理了）。
fn copy_to_temp(src: &Path) -> std::io::Result<Option<PathBuf>> {
    let name = format!(
        "ai-manager-ima-{}-{}.db",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let temp = std::env::temp_dir().join(name);
    match std::fs::copy(src, &temp) {
        Ok(_) => Ok(Some(temp)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

fn cookie_source(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Cookies")
        .to_string()
}

fn read_cookie_pairs(path: &Path, key: &Key<Aes256Gcm>) -> Result<Vec<(String, String)>> {
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("读取 ima Cookie 库失败：{e}")))?;
    let mut stmt = conn
        .prepare("SELECT name, value, encrypted_value FROM cookies")
        .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("读取 ima Cookie 表失败：{e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })
        .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("查询 ima Cookie 失败：{e}")))?;

    let mut out = Vec::new();
    for row in rows {
        let (name, value, encrypted) = row.map_err(|e| {
            AppError::new(ErrorKind::Sqlite, format!("读取 ima Cookie 行失败：{e}"))
        })?;
        if !REQUIRED_COOKIES.contains(&name.as_str()) {
            continue;
        }
        let text = match value.filter(|v| !v.trim().is_empty()) {
            Some(value) => value,
            None => match encrypted.filter(|blob| !blob.is_empty()) {
                Some(blob) => decode_cookie_value(key, &blob)?,
                None => continue,
            },
        };
        if !text.is_empty() {
            out.push((name, text));
        }
    }
    Ok(out)
}

fn decode_cookie_value(key: &Key<Aes256Gcm>, blob: &[u8]) -> Result<String> {
    if blob.starts_with(b"v20") {
        return Err(AppError::new(
            ErrorKind::Unsupported,
            "ima Cookie 使用应用绑定加密（v20），本工具暂不支持；请在 ima 客户端保持登录，等待客户端版本更新后重试",
        ));
    }
    let plain = crate::app::secret::chromium_open(key, blob)?;
    // 新版 Chromium 会在明文前加 32 字节完整性前缀（host_key 的摘要）。
    // 长度 > 32 才剥离：旧版客户端没有前缀，短值原样使用。
    let value = if plain.len() > 32 {
        &plain[32..]
    } else {
        &plain[..]
    };
    String::from_utf8(value.to_vec())
        .map_err(|_| AppError::new(ErrorKind::Crypto, "ima Cookie 解密结果不是 UTF-8"))
}

// ------------------------------------------------------------------ 协议小工具

/// bkn 签名：djb2 变体。按 int32 回绕累加，最后清掉符号位。
///
/// 用 `wrapping_*` 而不是在每步 `& 0x7FFFFFFF`：脚本原实现是 int32 溢出回绕，
/// 每步掩码会让结果不同（固定值见测试）。
fn bkn(token: &str) -> String {
    let mut hash: i32 = 5381;
    for ch in token.chars() {
        hash = hash.wrapping_mul(33).wrapping_add(ch as i32);
    }
    ((hash & 0x7fff_ffff) as u32).to_string()
}

fn json_uid(uid: &str) -> Value {
    uid.parse::<i64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::String(uid.to_string()))
}

fn as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|v| v as i64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

fn as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// 余额字段是字符串；`display_*` 给界面直接展示，可能带千分位。
fn parse_balance(value: &Value) -> Option<f64> {
    value
        .get("balance")
        .and_then(as_amount)
        .or_else(|| value.get("display_amount").and_then(as_amount))
}

fn as_amount(value: &Value) -> Option<f64> {
    match value {
        Value::String(text) => text.trim().replace(',', "").parse().ok(),
        other => other.as_f64(),
    }
}

fn balance_text(balance: Option<f64>) -> String {
    match balance {
        Some(balance) => format!("算力 {}", trim_f64(balance)),
        None => "算力暂未返回".into(),
    }
}

fn login_expired() -> AppError {
    AppError::new(
        ErrorKind::AuthExpired,
        "ima 登录已失效，请打开 ima 客户端确认登录后重新扫描（或重新登录）",
    )
}

fn http_error(action: &str, status: u16) -> AppError {
    match status {
        401 | 403 => login_expired(),
        429 => AppError::new(ErrorKind::RateLimited, "ima 接口限流，请稍后再试"),
        _ => AppError::new(
            ErrorKind::Provider,
            format!("{action}失败（HTTP {status}）"),
        ),
    }
}

// ------------------------------------------------------------------ 测试

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::net::MockHttp;
    use rusqlite::params;

    fn provider() -> ImaProvider {
        ImaProvider::at(std::env::temp_dir().join("ai-manager-ima-no-such-dir"))
    }

    fn cred() -> Credential {
        let mut cred = Credential {
            refresh_token: "rt-live".into(),
            ..Default::default()
        };
        cred.put("ima_uid", json!("1234567890123456"));
        cred.put("uid_type", json!("2"));
        cred.put("ima_guid", json!("guid-abc"));
        cred
    }

    fn activity(today: i64, full_week: i64, days: i64, total: f64) -> String {
        format!(
            r#"{{"code":0,"infos":[{{"top":"今日","status":{today}}},{{"top":"满签奖励","status":{full_week}}}],"checkin_days":{days},"total_reward_points":{total}}}"#
        )
    }

    /// 「我的余额」接口：`total_reward_points`（活动累计）不应被当成这个值。
    fn balance(amount: &str) -> String {
        format!(
            r#"{{"code":0,"balance":"{amount}","display_amount":"{amount}","display_activity_amount":"{amount}","display_paid_amount":"0","real_balance":"0","balance_infos":[]}}"#
        )
    }

    #[tokio::test]
    async fn status_takes_balance_from_token_manager_not_activity_total() {
        let http = MockHttp::new();
        http.on(REFRESH_URL, 200, r#"{"code":0,"token":"tk-live"}"#);
        // 活动累计只有 120，当前可用余额 650：界面必须显示后者
        http.on(ACTIVITY_URL, 200, &activity(2, 4, 5, 120.0));
        http.on(BALANCE_URL, 200, &balance("650"));

        let status = provider().status(&cred(), &http).await.unwrap();
        assert_eq!(status.checked_in, Some(true));
        assert_eq!(status.balance, Some(650.0));
        assert!(
            status.summary.contains("算力 650"),
            "{}",
            status.summary
        );
        assert!(status.summary.contains("已签到 5 天"), "{}", status.summary);

        // access token 只用于当次请求；refresh 的 x-ima-cookie 只带 GUID
        assert_eq!(
            http.last_header(REFRESH_URL, "x-ima-cookie").as_deref(),
            Some("IMA-GUID=guid-abc; PLATFORM=H5")
        );
        let activity_cookie = http.last_header(ACTIVITY_URL, "x-ima-cookie").unwrap();
        assert!(activity_cookie.contains("IMA-TOKEN=tk-live"));
        assert!(activity_cookie.contains("UID-TYPE=2"));
        assert!(activity_cookie.contains("PLATFORM=H5"));
        assert_eq!(
            http.last_header(ACTIVITY_URL, "x-ima-bkn").as_deref(),
            Some(bkn("tk-live").as_str())
        );
        assert_eq!(
            http.last_header(ACTIVITY_URL, "from_browser_ima")
                .as_deref(),
            Some("1")
        );
        assert_eq!(http.last_body(ACTIVITY_URL).as_deref(), Some("{}"));
        // 余额接口同样要带签名与登录态，且 body 是空对象
        assert_eq!(
            http.last_header(BALANCE_URL, "x-ima-bkn").as_deref(),
            Some(bkn("tk-live").as_str())
        );
        assert!(http
            .last_header(BALANCE_URL, "x-ima-cookie")
            .is_some_and(|value| value.contains("IMA-TOKEN=tk-live")));
        assert_eq!(http.last_body(BALANCE_URL).as_deref(), Some("{}"));
    }

    #[tokio::test]
    async fn checkin_posts_type_one_and_reads_reward() {
        let http = MockHttp::new();
        http.on(REFRESH_URL, 200, r#"{"code":0,"token":"tk-live"}"#);
        http.on_sequence(
            ACTIVITY_URL,
            vec![
                (200, &activity(3, 4, 4, 100.0)),
                (200, &activity(2, 4, 5, 110.0)),
            ],
        );
        http.on(CHECKIN_URL, 200, r#"{"code":0,"reward_received":10}"#);
        http.on(BALANCE_URL, 200, &balance("650"));

        let result = provider().checkin(&cred(), &http).await.unwrap();
        assert!(result.ok);
        assert!(!result.already);
        assert_eq!(result.granted, Some(10.0));
        assert!(
            result.message.contains("算力 650"),
            "{}",
            result.message
        );
        assert_eq!(http.call_count(CHECKIN_URL), 1);
        assert_eq!(
            http.last_body(CHECKIN_URL).as_deref(),
            Some(r#"{"type":1}"#)
        );
        assert!(http
            .last_header(CHECKIN_URL, "x-ima-bkn")
            .is_some_and(|value| !value.is_empty()));
    }

    #[tokio::test]
    async fn already_checked_in_does_not_call_the_checkin_endpoint() {
        let http = MockHttp::new();
        http.on(REFRESH_URL, 200, r#"{"code":0,"token":"tk-live"}"#);
        http.on(ACTIVITY_URL, 200, &activity(2, 4, 5, 120.0));
        http.on(BALANCE_URL, 200, &balance("650"));

        let result = provider().checkin(&cred(), &http).await.unwrap();
        assert!(result.ok);
        assert!(result.already);
        assert_eq!(result.granted, None);
        assert!(
            result.message.contains("算力 650"),
            "{}",
            result.message
        );
        assert_eq!(http.call_count(CHECKIN_URL), 0);
    }

    #[tokio::test]
    async fn refresh_failure_is_auth_expired_with_relogin_hint() {
        for body in [r#"{"code":1001}"#, r#"{"code":0}"#] {
            let http = MockHttp::new();
            http.on(REFRESH_URL, 200, body);
            let err = provider().status(&cred(), &http).await.unwrap_err();
            assert_eq!(err.kind, ErrorKind::AuthExpired);
            assert!(err.message.contains("重新登录"), "{}", err.message);
            assert!(err.message.contains("重新扫描"), "{}", err.message);
        }
    }

    #[tokio::test]
    async fn forbidden_checkin_is_reported_without_panic() {
        let http = MockHttp::new();
        http.on(REFRESH_URL, 200, r#"{"code":0,"token":"tk-live"}"#);
        http.on(ACTIVITY_URL, 200, &activity(3, 4, 4, 100.0));
        http.on(CHECKIN_URL, 403, "");

        let err = provider().checkin(&cred(), &http).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
        assert!(err.message.contains("403"), "{}", err.message);
    }

    #[test]
    fn bkn_matches_the_reference_implementation() {
        // 固定值由 int32 回绕 + 最终 & 0x7FFFFFFF 的参考实现算出
        assert_eq!(bkn("abc"), "193485963");
        assert_eq!(bkn("IMA-TOKEN-EXAMPLE-123"), "1856535622");
        assert_eq!(bkn("token123"), "1795221148");
        assert_eq!(bkn(""), "5381");
    }

    fn test_key() -> Key<Aes256Gcm> {
        *Key::<Aes256Gcm>::from_slice(&[5u8; 32])
    }

    fn sealed(key: &Key<Aes256Gcm>, value: &[u8], v11: bool) -> Vec<u8> {
        // 真实客户端明文前有 32 字节完整性前缀，这里照同样的形状造
        let mut plain = vec![0x3au8; 32];
        plain.extend_from_slice(value);
        let mut blob = crate::app::secret::chromium_seal(key, &plain).unwrap();
        if v11 {
            blob[..3].copy_from_slice(b"v11");
        }
        blob
    }

    #[test]
    fn reads_five_required_cookies_with_v10_v11_and_prefix() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("Cookies");
        let key = test_key();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB)",
        )
        .unwrap();
        let rows = [
            ("IMA-UID", sealed(&key, b"1234567890123456", true)),
            ("UID-TYPE", sealed(&key, b"2", false)),
            ("IMA-GUID", sealed(&key, b"guid-abc", false)),
            ("IMA-TOKEN", sealed(&key, b"tk-live", false)),
            ("IMA-REFRESH-TOKEN", sealed(&key, b"rt-live", true)),
        ];
        for (name, blob) in rows {
            conn.execute(
                "INSERT INTO cookies (host_key, name, value, encrypted_value) VALUES (?1, ?2, '', ?3)",
                params!["ima.qq.com", name, blob],
            )
            .unwrap();
        }
        drop(conn);

        let pairs = read_cookie_pairs(&path, &key).unwrap();
        assert_eq!(pairs.len(), 5, "5 个必需字段都要取到");
        let set = CookieSet::from_pairs(pairs, "Cookies".into()).unwrap();
        assert_eq!(set.ima_uid, "1234567890123456");
        assert_eq!(set.uid_type, "2");
        assert_eq!(set.ima_guid, "guid-abc");
        assert_eq!(set.refresh_token, "rt-live");
        assert_eq!(set.source, "Cookies");
    }

    #[test]
    fn v20_cookie_reports_unsupported_instead_of_panicking() {
        let key = test_key();
        let mut blob = b"v20".to_vec();
        blob.extend_from_slice(&[0u8; 24]);
        let err = decode_cookie_value(&key, &blob).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported);
        assert!(err.message.contains("不支持"), "{}", err.message);
    }

    #[test]
    fn provider_is_registered_and_scannable() {
        let ima = ImaProvider::new();
        assert_eq!(ima.platform(), "ima");
        assert_eq!(ima.label(), "ima");
        assert!(ima.requires_checkin());
        assert!(ima.scannable());
        let registered = crate::app::providers::for_platform("ima").unwrap();
        assert_eq!(registered.platform(), "ima");
    }
}
