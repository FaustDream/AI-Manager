//! ZCode：登录态就是一份 JSON（`credentials.json` + `config.json`），整份存、整份写回。
//!
//! 字段值带 `enc:v1:` 或 `enc:v2:` 前缀（客户端 3.18 起 v1 整体迁移成 v2）：
//!   - v1：密钥由「平台 + 家目录 + 用户名」推导（不落盘、不随账号变）；
//!   - v2：`enc:v2:<os|env>:<指纹24hex>:<payload>`，os 源密钥放在 Windows 凭据管理器
//!     （keyring v3 后端，目标名 `<sha256(credentials.json 路径)hex>.<服务名>`），
//!     env 源密钥 = sha256(共享串)。payload 同为 base64url `nonce.tag.cipher` 三段，
//!     AES-256-GCM 12/16/无 AAD。
//!
//! 因此**原始密文可以原样搬运**：换账号只需要把那两份 JSON 写回去，
//! 不需要解密再重新加密——这也避免了「重新加密后密钥不一致」这类问题。
//!
//! 设备身份（telemetry 的 `deviceMid` 与 `rum-electron-store` 里的 `_arms_uid`）
//! 跟着账号走：不同账号共用同一个设备号会被服务端算成同一个人，额度就串了。

use super::{Provider, ScannedAccount, Sink, SwitchOutcome};
use crate::app::accounts::credential::{iso_from_ms, Credential};
use crate::app::accounts::AccountIdentity;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::app::{fsx, paths, process};
use crate::types::{CreditSnapshot, StatusResult};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::Aes256Gcm;
use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const PREFIX: &str = "enc:v1:";
const PREFIX_V2: &str = "enc:v2:";
/// 客户端把 v2:os 密钥存进 Windows 凭据管理器用的服务名（asar 逆向得出）
const KEYRING_SERVICE: &str = "ZCode Credential Encryption";

pub struct ZCodeProvider {
    /// ZCode 的 dataBaseDir（`~/.zcode` 的父目录）
    data_root: PathBuf,
    /// `%APPDATA%`，设备指纹在它下面的 ZCode 目录里
    appdata: Option<PathBuf>,
    /// 客户端全局设置。固定在家目录，**不跟随 dataBaseDir**（v1 的实测结论）
    setting: PathBuf,
    /// 进程探测的注入点：`None` = 真去查全局进程表（生产路径）。
    /// 不注入的话单元测试会跟着本机是否开着客户端一起红绿，CI 上必炸。
    alive: Option<bool>,
}

impl ZCodeProvider {
    pub fn new() -> Self {
        Self {
            data_root: default_data_root(),
            appdata: std::env::var("APPDATA").ok().map(PathBuf::from),
            setting: paths::home().join(".zcode").join("v2").join("setting.json"),
            alive: None,
        }
    }

    pub fn at(data_root: PathBuf, appdata: Option<PathBuf>) -> Self {
        let setting = data_root.join(".zcode").join("v2").join("setting.json");
        Self {
            data_root,
            appdata,
            setting,
            alive: None,
        }
    }

    /// 测试专用：固定「客户端是否在跑」，不碰真实进程表。
    pub fn assume_alive(mut self, value: bool) -> Self {
        self.alive = Some(value);
        self
    }

    /// 客户端是否在运行。生产路径走真实探测，测试由 `assume_alive` 覆盖。
    fn client_running(&self) -> bool {
        self.alive
            .unwrap_or_else(|| process::process_running(&["ZCode.exe", "zcode.exe"]))
    }

    fn v2(&self) -> PathBuf {
        self.data_root.join(".zcode").join("v2")
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.v2().join("credentials.json")
    }

    pub fn config_path(&self) -> PathBuf {
        self.v2().join("config.json")
    }

    fn telemetry_path(&self) -> PathBuf {
        self.v2().join("telemetry-state.json")
    }

    fn plan_cache_path(&self) -> PathBuf {
        self.v2().join("coding-plan-cache.json")
    }

    /// 客户端的全局设置，`align_family_domain` 会写它
    fn setting_path(&self) -> PathBuf {
        self.setting.clone()
    }

    fn read_json(path: &Path) -> Option<Value> {
        let raw = std::fs::read(path).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    pub fn live_credentials(&self) -> Option<Value> {
        Self::read_json(&self.credentials_path())
    }

    fn device_mid(&self) -> Option<String> {
        let data = Self::read_json(&self.telemetry_path())?;
        data.get("deviceMid")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    fn arms_files(&self) -> Vec<PathBuf> {
        let Some(appdata) = self.appdata.as_ref().filter(|p| p.is_dir()) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = ["ZCode", "zcode"]
            .iter()
            .map(|n| appdata.join(n).join("rum-electron-store"))
            .filter(|p| p.is_dir())
            .collect();
        for entry in std::fs::read_dir(appdata).into_iter().flatten().flatten() {
            let store = entry.path().join("rum-electron-store");
            let lower = entry.file_name().to_string_lossy().to_lowercase();
            if lower.starts_with("zcode") && store.is_dir() && !dirs.contains(&store) {
                dirs.push(store);
            }
        }
        let mut files = Vec::new();
        for dir in dirs {
            let mut inner: Vec<PathBuf> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
                .collect();
            // 默认文件优先，与 v1 的排序规则一致
            inner.sort_by_key(|p| {
                p.file_name()
                    .map(|n| n != "ZGVmYXVsdA.json")
                    .unwrap_or(true)
            });
            files.extend(inner);
        }
        files
    }

    fn arms_uid(&self) -> Option<String> {
        self.arms_files().into_iter().find_map(|path| {
            let data = Self::read_json(&path)?;
            data.get("_arms_uid")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
    }
}

impl Default for ZCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------ 加解密

/// v1 密钥口径：`zcode-credential-fallback:win32:{家目录}:{用户名}`。
/// 不能换成 DPAPI——那是本工具自己的账号库在用，与 ZCode 无关。
fn secret() -> String {
    if let Ok(env) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        if !env.is_empty() {
            return env;
        }
    }
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".into());
    format!(
        "zcode-credential-fallback:win32:{}:{user}",
        paths::home().display()
    )
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn b64(text: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("不是合法 base64url：{e}")))
}

/// v1 是客户端 3.18 之前的旧格式，只解不写——新写盘一律是 v2，客户端自己会迁移。
fn v1_key() -> [u8; 32] {
    Sha256::digest(secret().as_bytes()).into()
}

/// v2:env 的共享串：`ZCODE_CREDENTIAL_SECRET` 环境变量，或凭据同目录的
/// `credential-secret` 文件（客户端要求 32-4096 字节）。密钥 = sha256(共享串)。
fn v2_env_secret(creds_dir: &Path) -> Option<Vec<u8>> {
    if let Ok(env) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        if !env.is_empty() {
            return Some(env.into_bytes());
        }
    }
    let raw = std::fs::read(creds_dir.join("credential-secret")).ok()?;
    if !(32..=4096).contains(&raw.len()) {
        return None;
    }
    Some(raw)
}

/// v2:os 的密钥：Windows 凭据管理器（客户端用 keyring v3 的 Windows 后端）。
/// 目标名 = `<sha256(credentials.json 绝对路径) hex>.<服务名>`——**user 在前、
/// 服务在后**（2026-11-14 本机实测）；Blob 是 43 位 base64url 文本，以 UTF-16LE
/// 存放，解出 32 字节原始密钥（不是 sha256，密钥就是这 32 字节本身）。
#[cfg(windows)]
fn v2_os_key(creds_path: &Path) -> Option<Vec<u8>> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::CRED_TYPE_GENERIC;
    use windows::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW};

    let user = sha256_hex(creds_path.to_string_lossy().as_bytes());
    let target = format!("{user}.{KEYRING_SERVICE}");
    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut cred_ptr: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe { CredReadW(PCWSTR(wide.as_ptr()), CRED_TYPE_GENERIC, 0, &mut cred_ptr) }.ok()?;
    if cred_ptr.is_null() {
        return None;
    }
    let cred = unsafe { &*cred_ptr };
    let blob = unsafe {
        std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
    };
    let units: Vec<u16> = blob
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let text = String::from_utf16_lossy(&units);
    unsafe { CredFree(cred_ptr.cast()) };
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim())
        .ok()?;
    if key.len() != 32 {
        return None;
    }
    Some(key)
}

#[cfg(not(windows))]
fn v2_os_key(_creds_path: &Path) -> Option<Vec<u8>> {
    None
}

/// 客户端在凭据同目录留的 `credential-source` 文件钉死了密钥模式（"os" 或 "env"）。
/// 客户端规则：值里声明的源必须与实际加载的源一致，否则按凭据不可用处理。
fn pinned_source(creds_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(creds_path.parent()?.join("credential-source")).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 生产密钥解析：按值的 source 段取 v2 密钥。
fn resolve_v2_key(source: &str, creds_path: &Path) -> Option<Vec<u8>> {
    if let Some(pinned) = pinned_source(creds_path) {
        if pinned != source {
            return None;
        }
    }
    match source {
        "os" => v2_os_key(creds_path),
        "env" => {
            let secret = v2_env_secret(creds_path.parent()?)?;
            Some(Sha256::digest(&secret).to_vec())
        }
        _ => None,
    }
}

/// 解开一个加密字段。`enc:v1:` 与 `enc:v2:<source>:<指纹>:<payload>` 都认；
/// 非加密值原样返回（客户端就是混着存的）。
pub fn decrypt(value: &str) -> Option<String> {
    decrypt_with(value, &resolve_v2_key)
}

/// resolver 可注入的版本：单元测试用它假装一个凭据管理器。
pub fn decrypt_with(
    value: &str,
    resolve: &dyn Fn(&str, &Path) -> Option<Vec<u8>>,
) -> Option<String> {
    let creds_path = default_data_root()
        .join(".zcode")
        .join("v2")
        .join("credentials.json");
    if let Some(body) = value.strip_prefix(PREFIX) {
        return open_triple(body, &v1_key());
    }
    if let Some(rest) = value.strip_prefix(PREFIX_V2) {
        let mut segs = rest.splitn(3, ':');
        let source = segs.next().unwrap_or("");
        let fingerprint = segs.next().unwrap_or("");
        let payload = segs.next().unwrap_or("");
        if fingerprint.len() != 24 || !fingerprint.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let key = resolve(source, &creds_path)?;
        // 指纹 = sha256(密钥) 的前 24 个 hex：对不上说明不是这把钥匙，别硬解
        if &sha256_hex(&key)[..24] != fingerprint {
            return None;
        }
        return open_triple(payload, &key);
    }
    Some(value.to_string())
}

fn open_triple(payload: &str, key: &[u8]) -> Option<String> {
    let parts: Vec<&str> = payload.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let nonce = b64(parts[0]).ok()?;
    let tag = b64(parts[1]).ok()?;
    let mut cipher = b64(parts[2]).ok()?;
    cipher.extend_from_slice(&tag);
    let plain = Aes256Gcm::new(aes_gcm::Key::<Aes256Gcm>::from_slice(key))
        .decrypt(aes_gcm::Nonce::from_slice(&nonce), cipher.as_ref())
        .ok()?;
    String::from_utf8(plain).ok()
}

fn field(creds: &Value, key: &str) -> Option<String> {
    decrypt(creds.get(key)?.as_str()?)
}

fn json_field(creds: &Value, key: &str) -> Option<Value> {
    serde_json::from_str(&field(creds, key)?).ok()
}

fn jwt_payload(token: &str) -> Map<String, Value> {
    let Some(part) = token.split('.').nth(1) else {
        return Map::new();
    };
    b64(part)
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn is_logged_in(creds: &Value) -> bool {
    let Some(obj) = creds.as_object() else {
        return false;
    };
    let oauth = obj
        .keys()
        .any(|k| k.starts_with("oauth:") && k.ends_with(":access_token"));
    let token = obj
        .get("zcodejwttoken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    oauth || !token.is_empty()
}

/// 三个登录族。zai_cn = z.ai 中国区（授权必须走服务端下发 URL，客户端源码同口径）。
const FAMILIES: [&str; 3] = ["bigmodel", "zai", "zai_cn"];

/// 列表首列的平台标签按族区分，智谱与国际/中国 z.ai 一眼可分。
pub fn family_label(family: &str) -> &'static str {
    match family {
        "zai" => "ZCode z.ai",
        "zai_cn" => "ZCode z.ai CN",
        _ => "ZCode",
    }
}

/// 登录族：`oauth:active_provider` 解出来直接用；缺了按存在的 access_token 键推断；
/// 再兜 bigmodel（存量行为）。identity 与 candidate_tokens 共用这一个口径。
fn family_of(creds: &Value) -> String {
    if let Some(active) = field(creds, "oauth:active_provider") {
        if FAMILIES.contains(&active.as_str()) {
            return active;
        }
    }
    for fam in FAMILIES {
        if creds.get(format!("oauth:{fam}:access_token")).is_some() {
            return fam.to_string();
        }
    }
    "bigmodel".to_string()
}

/// 账号身份。返回 (account_id, 昵称, 到期 ms, 族, email)
///
/// account_id 不再落到显示名占位串：user_id → access_token JWT sub →
/// zcodejwttoken JWT sub → email → token 指纹。否则一次匿名登录就会
/// 生成 `zcode:ZCode 账号` 这种永续的重复行。zai/zai_cn 族加区域后缀——
/// 不同族的 uid 可能相同（zai_cn 与 bigmodel 关联账号实测同 uid），不加会互相覆盖。
pub fn identity(creds: &Value) -> (String, String, Option<i64>, String, String) {
    let family = family_of(creds);
    let info = json_field(creds, &format!("oauth:{family}:user_info")).unwrap_or(Value::Null);
    let mut user_id = info.get("id").and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    if user_id.is_none() {
        let access = field(creds, &format!("oauth:{family}:access_token")).unwrap_or_default();
        let payload = jwt_payload(&access);
        user_id = payload
            .get("user_id")
            .or_else(|| payload.get("sub"))
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            });
    }
    if user_id.is_none() {
        let payload = field(creds, "zcodejwttoken")
            .map(|t| jwt_payload(&t))
            .unwrap_or_default();
        user_id = payload
            .get("user_id")
            .or_else(|| payload.get("sub"))
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            });
    }
    let email = info
        .get("email")
        .and_then(|v| v.as_str())
        .or_else(|| info.pointer("/rawProfile/email").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let name = info
        .get("displayName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            info.get("username")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .map(str::to_string)
        .unwrap_or_else(|| {
            // 手机号登录的账号，客户端把手机号写成 xxx@phone.local，只留号码
            email.split('@').next().unwrap_or("").to_string()
        })
        .trim()
        .to_string();
    let name = if name.is_empty() {
        // 只做显示兜底，绝不进 account_id
        "ZCode 账号".to_string()
    } else {
        name
    };

    let expires = field(creds, "zcodejwttoken")
        .and_then(|token| jwt_payload(&token).get("exp").and_then(|v| v.as_i64()))
        .map(|sec| sec * 1000);
    let base = if !user_id.as_deref().unwrap_or("").is_empty() {
        user_id.unwrap()
    } else if !email.is_empty() {
        email.clone()
    } else {
        // 最后兜底：token 指纹。同一份 token 稳定对应同一行，重登同一账号不会刷出新行
        let token = field(creds, "zcodejwttoken")
            .or_else(|| field(creds, &format!("oauth:{family}:access_token")))
            .unwrap_or_default();
        if token.is_empty() {
            "unknown".to_string()
        } else {
            sha256_hex(token.as_bytes())[..12].to_string()
        }
    };
    let account_id = match family.as_str() {
        "zai" => format!("{base}@zai"),
        "zai_cn" => format!("{base}@zai_cn"),
        _ => base,
    };
    (account_id, name, expires, family, email)
}

#[async_trait]
impl Provider for ZCodeProvider {
    fn platform(&self) -> &'static str {
        "zcode"
    }

    fn label(&self) -> &'static str {
        "ZCode"
    }

    fn scan(&self, sink: &mut Sink<'_>) -> Result<usize> {
        let Some(creds) = self.live_credentials() else {
            return Ok(0);
        };
        if !is_logged_in(&creds) {
            return Ok(0);
        }
        let (account_id, name, expires_at, provider, email) = identity(&creds);
        let config = Self::read_json(&self.config_path()).unwrap_or(Value::Null);

        let mut cred = Credential {
            token: field(&creds, "zcodejwttoken").unwrap_or_default(),
            expires_at: expires_at.map(iso_from_ms).unwrap_or_default(),
            ..Default::default()
        };
        // 整份凭据原样存（含 enc:v1 密文）：写回时不需要重新加密
        cred.put("credentials", creds.clone());
        cred.put("config", config);
        cred.put("uid", json!(account_id));
        cred.put("name", json!(name));
        cred.put("provider", json!(provider));
        cred.put("email", json!(email));
        // 账号自己的 UA：以后换客户端版本只改这里，不必重扫账号
        cred.put("ua", json!(format!("ZCode/{APP_VERSION}")));
        if let Some(mid) = self.device_mid() {
            cred.put("device_mid", json!(mid));
        }
        if let Some(arms) = self.arms_uid() {
            cred.put("arms_uid", json!(arms));
        }

        let identity = AccountIdentity {
            iid: format!("zcode:{account_id}"),
            platform: "zcode".into(),
            platform_label: family_label(&provider).to_string(),
            account_id,
            name,
            expires_at,
            source: "credentials.json".into(),
            // ZCode 没有签到玩法，只有额度窗口
            requires_checkin: false,
            has_secret: true,
        };
        sink(ScannedAccount::new(identity, &cred)?)?;
        Ok(1)
    }

    fn current_id(&self) -> Option<String> {
        let creds = self.live_credentials()?;
        if !is_logged_in(&creds) {
            return None;
        }
        Some(identity(&creds).0)
    }

    // ------------------------------------------------------------ P3：额度（无签到）

    /// ZCode 没有签到玩法：只查额度，界面上显示额度条而不是积分
    fn requires_checkin(&self) -> bool {
        false
    }

    async fn status(&self, cred: &Credential, http: &dyn Http) -> Result<StatusResult> {
        let snapshot = self.credits(cred, http).await?;
        Ok(StatusResult {
            checked_in: None,
            summary: snapshot.summary,
            balance: None,
            lots: None,
            next_at: None,
        })
    }

    async fn credits(&self, cred: &Credential, http: &dyn Http) -> Result<CreditSnapshot> {
        let quota = self.quota(cred, http).await?;
        Ok(CreditSnapshot {
            summary: quota.summary,
            error: None,
            balance: None,
            lots: None,
            quota: Some(quota.windows),
        })
    }
}

/// 归一化后的额度：一句话摘要 + 各窗口已用百分比（前端画额度条用）。
struct QuotaView {
    summary: String,
    windows: std::collections::HashMap<String, f64>,
}

const LIMIT_URL: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
const SUBSCRIPTION_URL: &str = "https://open.bigmodel.cn/api/biz/subscription/list";
const BALANCE_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/balance";
/// 客户端版本号。活动接口的 app_version 参数与这里保持同源（zclaim 用）。
pub(crate) const APP_VERSION: &str = "3.16.3";

impl ZCodeProvider {
    /// 客户端真实会用的 token 顺序：JWT 优先，其次 provider 的 access token，
    /// 最后是 config.json 里 coding-plan 的 apiKey（新版放在这里）。
    pub(crate) fn candidate_tokens(cred: &Credential) -> Vec<String> {
        let mut tokens: Vec<String> = Vec::new();
        let mut add = |value: Option<&str>| {
            if let Some(value) = value {
                if value.trim().len() > 20 && !tokens.iter().any(|t| t == value) {
                    tokens.push(value.to_string());
                }
            }
        };
        let creds = cred.meta_json("credentials");
        let field = |key: &str| -> Option<String> {
            creds
                .and_then(|c| c.get(key))
                .and_then(|v| v.as_str())
                .and_then(decrypt)
        };
        let jwt = field("zcodejwttoken");
        add(jwt.as_deref());
        // 族口径与 identity 一致：active_provider 解不出就按键推断，别再各写各的默认值
        let provider = creds.map(family_of).unwrap_or_else(|| "bigmodel".into());
        for key in [
            format!("oauth:{provider}:access_token"),
            "oauth:bigmodel:access_token".to_string(),
            "oauth:zai:access_token".to_string(),
            "oauth:zai_cn:access_token".to_string(),
        ] {
            let value = field(&key);
            add(value.as_deref());
        }
        if let Some(providers) = cred.meta_json("config").and_then(|c| c.get("provider")) {
            for entry in providers.as_object().into_iter().flatten().map(|(_, v)| v) {
                let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if !name.contains("coding-plan") {
                    continue;
                }
                if let Some(key) = entry.pointer("/options/apiKey").and_then(|v| v.as_str()) {
                    if !key.starts_with("enc:") {
                        add(Some(key));
                    }
                }
            }
        }
        tokens
    }

    pub(crate) fn zai_headers(
        cred: &Credential,
        token: &str,
        mid: Option<&str>,
    ) -> Vec<(String, String)> {
        let mut headers = vec![
            (
                "User-Agent".to_string(),
                cred.user_agent(&format!("ZCode/{APP_VERSION}")),
            ),
            ("HTTP-Referer".into(), "https://zcode.z.ai".into()),
            ("X-Title".into(), "Z Code@electron".into()),
            ("X-ZCode-App-Version".into(), APP_VERSION.into()),
            ("X-Platform".into(), "win32-x64".into()),
            ("X-Release-Channel".into(), "stable".into()),
            ("X-Client-Language".into(), "zh-CN".into()),
            ("X-Client-Timezone".into(), "Asia/Shanghai".into()),
            ("X-Os-Category".into(), "windows".into()),
            ("Authorization".into(), format!("Bearer {token}")),
            ("x-request-id".into(), request_id()),
        ];
        let mid = mid
            .map(str::to_string)
            .or_else(|| cred.meta_str("device_mid").map(str::to_string));
        if let Some(mid) = mid.filter(|m| !m.is_empty()) {
            headers.push(("X-Device-Mid".into(), mid));
        }
        headers
    }

    fn bigmodel_headers(cred: &Credential, token: &str) -> Vec<(String, String)> {
        vec![
            ("Authorization".into(), format!("Bearer {token}")),
            (
                "User-Agent".into(),
                cred.user_agent(&format!("ZCode/{APP_VERSION}")),
            ),
            ("x-request-id".into(), request_id()),
        ]
    }

    /// GET + 429 退避重试。额度接口限流很常见，重试比报错有用。
    async fn get(&self, http: &dyn Http, url: &str, headers: &[(String, String)]) -> Result<Value> {
        let mut last = String::new();
        for delay in [0u64, 500, 1500, 4000] {
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            let req = HttpRequest::get(url).headers(headers);
            let resp = match http.send(req).await {
                Ok(resp) => resp,
                Err(e) => {
                    return Err(AppError::new(
                        ErrorKind::Provider,
                        format!("网络错误：{}", e.message),
                    ))
                }
            };
            if resp.status == 429 {
                last = "额度接口限流（HTTP 429）".into();
                continue;
            }
            if resp.status == 401 || resp.status == 403 {
                return Err(AppError::new(
                    ErrorKind::Provider,
                    format!("登录态已失效（HTTP {}）", resp.status),
                ));
            }
            if resp.status >= 400 {
                return Err(AppError::new(
                    ErrorKind::Provider,
                    format!("额度接口返回 HTTP {}", resp.status),
                ));
            }
            return resp.json();
        }
        Err(AppError::new(ErrorKind::Provider, last))
    }

    async fn quota(&self, cred: &Credential, http: &dyn Http) -> Result<QuotaView> {
        let tokens = Self::candidate_tokens(cred);
        if tokens.is_empty() {
            return Err(AppError::new(ErrorKind::Provider, "没有可用的登录凭据"));
        }
        // 族感知：z.ai 族（zai/zai_cn）的 token 在 BigModel 监控接口上只会 401 或
        // 「没有资格」，直接走余额通道——省一轮注定失败的请求，也不往错误列表里塞脏数据。
        // 只认**显式**的族标记：缺省（旧数据）保持 bigmodel 通道，行为不回退。
        if matches!(cred.meta_str("provider"), Some("zai") | Some("zai_cn")) {
            return self.quota_from_balance_channel(cred, http, &tokens).await;
        }
        let mut errors: Vec<String> = Vec::new();
        let mut auth_fail = 0;
        let mut no_plan = false;

        for token in &tokens {
            match self
                .get(http, LIMIT_URL, &Self::bigmodel_headers(cred, token))
                .await
            {
                Ok(data) if business_ok(&data) => {
                    let subscription = self
                        .get(http, SUBSCRIPTION_URL, &Self::bigmodel_headers(cred, token))
                        .await
                        .ok();
                    return Ok(quota_from_limit(&data, subscription.as_ref()));
                }
                Ok(data) => {
                    let code = data.get("code").and_then(|v| v.as_i64());
                    let message = data
                        .get("msg")
                        .or_else(|| data.get("message"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if code == Some(401) {
                        auth_fail += 1;
                    }
                    // 这个 token 在 BigModel 侧没有 coding plan，但 z.ai 账号本来就走
                    // 余额通道，不能就此判定「无额度」，继续往下试
                    if message.contains("coding plan") || message.contains("没有资格") {
                        no_plan = true;
                        continue;
                    }
                    let code_value = code.unwrap_or(-1);
                    errors.push(if message.is_empty() {
                        crate::app::providers::hints::with_hint(
                            format!("接口返回 code={code_value}"),
                            code_value,
                        )
                    } else {
                        crate::app::providers::hints::with_hint(message, code_value)
                    });
                }
                Err(e) => {
                    if e.message.contains("401") {
                        auth_fail += 1;
                    }
                    errors.push(e.message);
                }
            }
        }

        let all_auth_failed = auth_fail >= tokens.len() && !no_plan;
        if !all_auth_failed {
            if let Ok(view) = self.quota_from_balance_channel(cred, http, &tokens).await {
                return Ok(view);
            }
        }
        if no_plan || all_auth_failed {
            // 全部 token 都 401 时，余额通道对过期 token 也只是返回空数据，继续问下去
            // 只会把「没有套餐」显示成「读取失败」。结论按「无额度」给。
            return Ok(QuotaView {
                summary: if all_auth_failed {
                    "无额度数据（登录态可能已过期）".into()
                } else {
                    "Free · 无额度数据".into()
                },
                windows: std::collections::HashMap::new(),
            });
        }
        Err(AppError::new(
            ErrorKind::Provider,
            errors
                .into_iter()
                .next()
                .unwrap_or_else(|| "额度查询失败".into()),
        ))
    }

    /// z.ai 族（zai/zai_cn）的额度：只有余额通道。所有 token 试一遍，全失败才报错。
    async fn quota_from_balance_channel(
        &self,
        cred: &Credential,
        http: &dyn Http,
        tokens: &[String],
    ) -> Result<QuotaView> {
        let mut errors: Vec<String> = Vec::new();
        for token in tokens {
            let url = format!("{BALANCE_URL}?app_version={APP_VERSION}");
            match self
                .get(http, &url, &Self::zai_headers(cred, token, None))
                .await
            {
                Ok(data) if business_ok(&data) => return Ok(quota_from_balance(&data)),
                Ok(_) => {}
                Err(e) => errors.push(e.message),
            }
        }
        Err(AppError::new(
            ErrorKind::Provider,
            errors
                .into_iter()
                .next()
                .unwrap_or_else(|| "额度查询失败".into()),
        ))
    }
}

fn business_ok(data: &Value) -> bool {
    let code = data.get("code").and_then(|v| v.as_i64());
    if let Some(code) = code {
        if code != 0 && code != 200 {
            return false;
        }
    }
    data.get("success").and_then(|v| v.as_bool()) != Some(false)
}

/// BigModel：`limits[]` 里是 5 小时窗口 / 周额度。
fn quota_from_limit(data: &Value, subscription: Option<&Value>) -> QuotaView {
    let body = data.get("data").cloned().unwrap_or_else(|| json!({}));
    let mut windows = std::collections::HashMap::new();
    let mut parts: Vec<String> = Vec::new();

    for entry in body
        .get("limits")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let kind = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let total = entry.get("usage").and_then(|v| v.as_f64());
        let used = entry.get("currentValue").and_then(|v| v.as_f64());
        let percent = match (total, used) {
            (Some(total), Some(used)) if total > 0.0 => {
                Some((used / total * 100.0).clamp(0.0, 100.0))
            }
            _ => entry.get("percentage").and_then(|v| v.as_f64()),
        };
        let window = window_label(
            entry.get("unit").and_then(|v| v.as_i64()).unwrap_or(0),
            entry.get("number").and_then(|v| v.as_i64()).unwrap_or(0),
        );
        let name = match kind {
            "TOKENS_LIMIT" => "Token 额度",
            "TIME_LIMIT" => "使用时长",
            other if !other.is_empty() => other,
            _ => "额度",
        };
        let label = if window.is_empty() {
            name.to_string()
        } else {
            format!("{name}（{window}）")
        };
        if let Some(percent) = percent {
            windows.insert(label.clone(), percent);
            parts.push(format!(
                "{}剩 {:.0}%",
                if window.is_empty() {
                    label.clone()
                } else {
                    window.clone()
                },
                100.0 - percent
            ));
        }
    }
    let mut tier = tier_from_level(body.get("level").and_then(|v| v.as_str()).unwrap_or(""));
    if let Some(rows) = subscription
        .filter(|s| business_ok(s))
        .and_then(|s| s.get("data"))
        .and_then(|v| v.as_array())
    {
        if let Some(current) = rows
            .iter()
            .find(|row| {
                row.get("status")
                    .and_then(|v| v.as_str())
                    .map(str::to_uppercase)
                    == Some("VALID".into())
            })
            .or_else(|| rows.first())
        {
            if let Some(name) = current.get("productName").and_then(|v| v.as_str()) {
                let mapped = tier_from_level(name);
                tier = if mapped.is_empty() {
                    name.to_string()
                } else {
                    mapped
                };
            }
        }
    }

    let summary = if parts.is_empty() {
        if tier.is_empty() {
            "无额度数据".to_string()
        } else {
            format!("{tier} · 无额度数据")
        }
    } else {
        let head = parts
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        if tier.is_empty() {
            head
        } else {
            format!("{tier} · {head}")
        }
    };
    QuotaView { summary, windows }
}

/// z.ai：`balances[]` 里各有总用量与剩余。
fn quota_from_balance(data: &Value) -> QuotaView {
    let body = data.get("data").filter(|v| v.is_object()).unwrap_or(data);
    let mut windows = std::collections::HashMap::new();
    let mut parts: Vec<String> = Vec::new();

    for entry in body
        .get("balances")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let total = entry.get("total_units").and_then(|v| v.as_f64());
        let used = entry.get("used_units").and_then(|v| v.as_f64());
        let Some(percent) = ({
            match (total, used) {
                (Some(total), Some(used)) if total > 0.0 => {
                    Some((used / total * 100.0).clamp(0.0, 100.0))
                }
                _ => None,
            }
        }) else {
            continue;
        };
        let name = entry
            .get("show_name")
            .or_else(|| entry.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("额度")
            .to_string();
        windows.insert(name.clone(), percent);
        parts.push(format!("{name}剩 {:.0}%", 100.0 - percent));
    }

    let mut tier = String::new();
    if let Some(plans) = body.get("plans").and_then(|v| v.as_array()) {
        if let Some(plan) = plans.iter().find(|p| {
            p.get("status")
                .and_then(|v| v.as_str())
                .map(str::to_lowercase)
                == Some("active".into())
        }) {
            let name = plan.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let mapped = tier_from_level(name);
            tier = if mapped.is_empty() {
                name.to_string()
            } else {
                mapped
            };
        }
    }

    let summary = if parts.is_empty() {
        "额度未提供".to_string()
    } else {
        let head = parts
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        if tier.is_empty() {
            head
        } else {
            format!("{tier} · {head}")
        }
    };
    QuotaView { summary, windows }
}

/// unit 1/3/5 + number 组合出「5 小时」这类人话窗口名。
fn window_label(unit: i64, count: i64) -> String {
    match unit {
        1 => {
            if count > 0 && count < 60 {
                format!("{count} 分钟")
            } else {
                "小时".into()
            }
        }
        3 => {
            if count > 0 {
                format!("{count} 小时")
            } else {
                "小时".into()
            }
        }
        5 => {
            if count > 0 {
                format!("{count} 天")
            } else {
                "天".into()
            }
        }
        _ => String::new(),
    }
}

fn tier_from_level(level: &str) -> String {
    let lowered = level.to_lowercase();
    for (needle, tier) in [
        ("max", "Max"),
        ("pro", "Pro"),
        ("lite", "Lite"),
        ("free", "Free"),
    ] {
        if lowered.contains(needle) {
            return tier.to_string();
        }
    }
    level.trim().to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `x-request-id`：客户端每次请求都带一个新 uuid，缺失会被网关当成异常流量。
fn request_id() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::getrandom(&mut bytes);
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16])
    )
}

// ------------------------------------------------------------------ 切换

/// 把一份档案写回客户端。调用方负责先确认客户端已关闭。
pub fn switch_to(
    provider: &ZCodeProvider,
    account_id: &str,
    name: &str,
    cred: &Credential,
) -> Result<SwitchOutcome> {
    let Some(creds) = cred.meta_json("credentials").filter(|v| v.is_object()) else {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "这条账号里没有 ZCode 登录凭据，无法切换",
        ));
    };
    if !is_logged_in(creds) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "登录凭据已经失效，请重新登录一次该账号",
        ));
    }
    if provider.current_id().as_deref() == Some(account_id) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "这个账号就是客户端当前登录的账号，不需要切换",
        ));
    }
    if provider.client_running() {
        return Err(AppError::new(
            ErrorKind::ClientRunning,
            "请先关闭 ZCode 客户端再切换",
        ));
    }

    let target = provider.credentials_path();
    // 写的是原始密文，不是解密后的明文：密钥由机器推导，重新加密反而可能与客户端不一致
    fsx::write_atomic(&target, &serde_json::to_vec_pretty(creds)?)?;

    if let Some(config) = cred.meta_json("config").filter(|v| v.is_object()) {
        let path = provider.config_path();
        fsx::write_atomic(&path, &serde_json::to_vec_pretty(config)?)?;
    }

    // 设备身份与账号绑定：不换的话两个账号在服务端是同一台设备，额度会串
    if let Some(mid) = cred.meta_str("device_mid").filter(|s| !s.is_empty()) {
        let path = provider.telemetry_path();
        let mut data = ZCodeProvider::read_json(&path).unwrap_or_else(|| json!({}));
        if let Some(obj) = data.as_object_mut() {
            obj.insert("deviceMid".into(), json!(mid));
            fsx::write_atomic(&path, &serde_json::to_vec_pretty(&data)?)?;
        }
    }
    if let Some(arms) = cred.meta_str("arms_uid").filter(|s| !s.is_empty()) {
        write_arms_uid(provider, arms)?;
    }

    // 套餐缓存里是上一个账号的额度，留着会让界面显示错人的剩余量
    if let Err(e) = std::fs::remove_file(provider.plan_cache_path()) {
        if e.kind() != std::io::ErrorKind::NotFound {
            crate::app::log::log(
                crate::app::log::Level::Warn,
                "zcode",
                format!("清套餐缓存失败，界面可能显示上一个账号的额度：{e}"),
            );
        }
    }
    align_family_domain(provider, creds);

    Ok(SwitchOutcome {
        message: format!("ZCode 已切换到 {name}"),
    })
}

fn write_arms_uid(provider: &ZCodeProvider, uid: &str) -> Result<()> {
    let mut files = provider.arms_files();
    if files.is_empty() {
        // 没有现成的指纹文件：不新建目录，只是「没有设备指纹」而不是失败
        return Ok(());
    }
    files.truncate(1);
    for path in files {
        let mut data = ZCodeProvider::read_json(&path).unwrap_or_else(|| json!({}));
        data["_arms_uid"] = json!(uid);
        if let Some(obj) = data.as_object_mut() {
            obj.remove("_arms_session"); // 旧会话留着会让服务端认成上一个账号
        }
        fsx::write_atomic(&path, &serde_json::to_vec_pretty(&data)?)?;
    }
    Ok(())
}

/// 客户端按 `setting.json` 里的 `providerFamilyDomain` 选域名族，
/// 不跟着凭据走会串站（国内凭据打到国际站点）。
fn align_family_domain(provider: &ZCodeProvider, creds: &Value) {
    let Some(active) = field(creds, "oauth:active_provider") else {
        return;
    };
    if active != "bigmodel" && active != "zai" {
        return;
    }
    let path = provider.setting_path();
    let Some(mut data) = ZCodeProvider::read_json(&path) else {
        return;
    };
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    obj.insert("providerFamilyDomain".into(), json!(active));
    obj.insert(
        "providerFamilyDomainUpdatedAt".into(),
        json!(crate::app::timeutil::now_ms()),
    );
    // 这是「国内凭据不串国际站」的防线，写失败不能无声——串站比报错更难排查
    let bytes = serde_json::to_vec_pretty(&data).unwrap_or_default();
    if let Err(e) = fsx::write_atomic(&path, &bytes) {
        crate::app::log::log(
            crate::app::log::Level::Warn,
            "zcode",
            format!("写 providerFamilyDomain 失败：{}", e.message),
        );
    }
}

fn default_data_root() -> PathBuf {
    if let Ok(raw) = std::env::var("ZCODE_DATA_BASE_DIR") {
        let path = PathBuf::from(raw.trim());
        if path.is_absolute() {
            return path;
        }
    }
    // 新版把 dataBaseDir 写进 ~/.zcode/v2/setting.json，旧版就是家目录
    if let Some(base) = ZCodeProvider::read_json(&paths::home().join(".zcode/v2/setting.json"))
        .and_then(|v| {
            v.get("dataBaseDir")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
        })
    {
        if base.is_absolute() {
            return base;
        }
    }
    paths::home()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seal(value: &str) -> String {
        let nonce = [3u8; 12];
        let sealed = Aes256Gcm::new(aes_gcm::Key::<Aes256Gcm>::from_slice(&v1_key()))
            .encrypt(aes_gcm::Nonce::from_slice(&nonce), value.as_bytes())
            .unwrap();
        let (cipher, tag) = sealed.split_at(sealed.len() - 16);
        format!(
            "{PREFIX}{}.{}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(tag),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cipher),
        )
    }

    /// 按客户端 v2 格式造一份值：`enc:v2:<source>:<指纹>:<nonce.tag.cipher>`
    fn seal_v2(value: &str, key: &[u8; 32]) -> String {
        let nonce = [5u8; 12];
        let sealed = Aes256Gcm::new(aes_gcm::Key::<Aes256Gcm>::from_slice(key))
            .encrypt(aes_gcm::Nonce::from_slice(&nonce), value.as_bytes())
            .unwrap();
        let (cipher, tag) = sealed.split_at(sealed.len() - 16);
        let fingerprint = &sha256_hex(key)[..24];
        format!(
            "{PREFIX_V2}os:{fingerprint}:{}.{}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(tag),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cipher),
        )
    }

    fn creds(user: &str) -> Value {
        json!({
            "oauth:active_provider": seal("bigmodel"),
            "oauth:bigmodel:user_info": seal(&json!({"id": user, "displayName": "慎静", "email": "a@b.c"}).to_string()),
            "oauth:bigmodel:access_token": seal("eyJhbGciOiJub25lIn0.eyJzdWIiOiJ4In0."),
            "zcodejwttoken": seal("eyJhbGciOiJub25lIn0.eyJleHAiOjE3MDAwMDAwMDB9."),
        })
    }

    #[test]
    fn encrypted_values_roundtrip() {
        let value = seal("17679130626");
        assert!(value.starts_with(PREFIX));
        assert_eq!(decrypt(&value).unwrap(), "17679130626");
        assert_eq!(decrypt("plain").unwrap(), "plain");
    }

    /// 客户端 3.18 起把值整体迁到 `enc:v2:os:`。密钥经注入的 resolver 提供
    ///（模拟凭据管理器），指纹校验必须拦下拿错钥匙的情况。
    #[test]
    fn v2_values_roundtrip_with_fingerprint_check() {
        let key = [7u8; 32];
        let value = seal_v2("zai_cn", &key);
        assert!(value.starts_with(PREFIX_V2));
        let resolve = |source: &str, _path: &Path| {
            if source == "os" {
                Some(key.to_vec())
            } else {
                None
            }
        };
        assert_eq!(decrypt_with(&value, &resolve).unwrap(), "zai_cn");

        // 指纹 = sha256(密钥) 前 24 hex：换一把钥匙必须解不开，而不是解出乱码
        let other = [9u8; 32];
        let wrong = |_source: &str, _path: &Path| Some(other.to_vec());
        assert!(decrypt_with(&value, &wrong).is_none());
    }

    /// credential-source 文件钉死密钥模式：内容就是 "os" 或 "env" 一行
    #[test]
    fn pinned_source_is_read_from_credentials_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("credential-source"), "env\n").unwrap();
        assert_eq!(
            pinned_source(&dir.path().join("credentials.json")).as_deref(),
            Some("env")
        );
        // 空目录：没有 credential-source 就是不钉模式
        let empty = tempfile::TempDir::new().unwrap();
        assert_eq!(pinned_source(&empty.path().join("credentials.json")), None);
    }

    #[test]
    fn garbage_is_rejected_without_panic() {
        assert!(decrypt("enc:v1:not.base64.at.all").is_none());
        assert!(decrypt("enc:v1:only-one-part").is_none());
    }

    #[test]
    fn identity_comes_from_user_info() {
        let (id, name, expires, provider, email) = identity(&creds("12345"));
        assert_eq!(id, "12345");
        assert_eq!(name, "慎静");
        assert_eq!(provider, "bigmodel");
        assert_eq!(email, "a@b.c");
        assert_eq!(expires, Some(1_700_000_000_000));
    }

    #[test]
    fn phone_login_shows_number_not_placeholder_domain() {
        let c = json!({
            "oauth:bigmodel:user_info": seal(&json!({"email": "17679130626@phone.local"}).to_string()),
        });
        let (_, name, _, _, _) = identity(&c);
        assert_eq!(
            name, "17679130626",
            "昵称是手机号；脱敏在视图层做，这里保留原值"
        );
    }

    /// z.ai 中国族的登录：user_info 里有真实昵称，account_id 带 @zai_cn 后缀——
    /// 同一个人在智谱和 z.ai 的 uid 相同（实测），不加后缀两个账号会互相覆盖
    #[test]
    fn zai_cn_identity_gets_region_suffix() {
        let c = json!({
            "oauth:active_provider": seal("zai_cn"),
            "oauth:zai_cn:user_info": seal(&json!({"id": "62951787128479523", "displayName": "旅行者0626", "email": "17679130626@phone.local"}).to_string()),
            "oauth:zai_cn:access_token": seal("tok"),
        });
        let (id, name, _, family, _) = identity(&c);
        assert_eq!(family, "zai_cn");
        assert_eq!(id, "62951787128479523@zai_cn");
        assert_eq!(name, "旅行者0626");
        assert_eq!(family_label("zai_cn"), "ZCode z.ai CN");
    }

    /// 同一 uid 在不同族必须得到不同 account_id（防跨族覆盖）
    #[test]
    fn same_uid_across_families_gets_distinct_ids() {
        let bigmodel = json!({
            "oauth:active_provider": seal("bigmodel"),
            "oauth:bigmodel:user_info": seal(&json!({"id": "123"}).to_string()),
        });
        let zai = json!({
            "oauth:active_provider": seal("zai"),
            "oauth:zai:user_info": seal(&json!({"id": "123"}).to_string()),
        });
        let (bm_id, ..) = identity(&bigmodel);
        let (zai_id, ..) = identity(&zai);
        assert_eq!(bm_id, "123");
        assert_eq!(zai_id, "123@zai");
    }

    /// 匿名登录（user_info 缺失、JWT 读不出 uid）时，account_id 落到 token 指纹：
    /// 同一份 token 稳定对应同一行，重登不会刷出新行；「ZCode 账号」只做显示兜底
    #[test]
    fn anonymous_login_uses_token_fingerprint_not_display_name() {
        let token = "not-even-a-jwt";
        let c = json!({
            "oauth:active_provider": seal("zai"),
            "oauth:zai:access_token": seal(token),
        });
        let (id, name, _, _, _) = identity(&c);
        assert_eq!(id, format!("{}@zai", &sha256_hex(token.as_bytes())[..12]));
        assert_eq!(name, "ZCode 账号");
    }

    /// z.ai 族凭据直接走余额通道：BigModel 监控接口一次都不该被请求（此前每轮
    /// 都先撞 401/无资格，把「没有套餐」误导成「查询失败」）
    #[tokio::test]
    async fn zai_family_quota_goes_straight_to_balance() {
        let http = crate::app::net::MockHttp::new();
        http.on(
            &format!("{BALANCE_URL}?app_version={APP_VERSION}"),
            200,
            r#"{"code":0,"balances":[{"total_units":100.0,"used_units":40.0}]}"#,
        );
        let mut cred = Credential {
            token: "tok-0123456789-abcdefghijklmnop".into(),
            ..Default::default()
        };
        cred.put("provider", json!("zai_cn"));
        cred.put(
            "credentials",
            json!({
                "zcodejwttoken": "tok-0123456789-abcdefghijklmnop",
                "oauth:zai_cn:access_token": "tok-0123456789-zyxwvutsrqponml",
            }),
        );

        let quota = ZCodeProvider::new().credits(&cred, &http).await.unwrap();

        assert_eq!(
            http.call_count(LIMIT_URL),
            0,
            "z.ai 族不该请求 BigModel 监控接口"
        );
        assert_eq!(
            http.call_count(&format!("{BALANCE_URL}?app_version={APP_VERSION}")),
            1
        );
        assert!(quota.summary.contains("剩"), "{}", quota.summary);
    }

    #[test]
    fn scan_reads_credentials_and_keeps_ciphertext() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".zcode/v2")).unwrap();
        std::fs::write(
            dir.path().join(".zcode/v2/credentials.json"),
            serde_json::to_vec(&creds("9")).unwrap(),
        )
        .unwrap();

        let provider = ZCodeProvider::at(dir.path().to_path_buf(), None);
        let mut rows = Vec::new();
        provider
            .scan(&mut |acc| {
                rows.push(acc);
                Ok(())
            })
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].identity.iid, "zcode:9");
        assert!(!rows[0].identity.requires_checkin);
        let stored = Credential::parse(&rows[0].secret).unwrap();
        let raw = stored.meta_json("credentials").unwrap();
        assert_eq!(
            raw.get("zcodejwttoken").unwrap().as_str().unwrap(),
            creds("9").get("zcodejwttoken").unwrap().as_str().unwrap(),
            "凭据必须原样保存（含 enc:v1 密文）"
        );
    }

    #[test]
    fn not_logged_in_returns_zero() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".zcode/v2")).unwrap();
        std::fs::write(dir.path().join(".zcode/v2/credentials.json"), b"{}").unwrap();
        let provider = ZCodeProvider::at(dir.path().to_path_buf(), None);
        assert_eq!(provider.scan(&mut |_| Ok(())).unwrap(), 0);
    }

    #[test]
    fn switch_rewrites_credentials_and_backs_up() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".zcode/v2")).unwrap();
        std::fs::write(
            dir.path().join(".zcode/v2/credentials.json"),
            b"{\"zcodejwttoken\":\"old\"}",
        )
        .unwrap();

        let provider = ZCodeProvider::at(dir.path().to_path_buf(), None).assume_alive(false);
        let mut cred = Credential::default();
        cred.put("credentials", creds("42"));
        cred.put("config", json!({"theme": "dark"}));

        switch_to(&provider, "42", "慎静", &cred).unwrap();
        let written: Value = serde_json::from_slice(
            &std::fs::read(dir.path().join(".zcode/v2/credentials.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(identity(&written).0, "42");
        assert!(dir.path().join(".zcode/v2/config.json").exists());
    }

    #[test]
    fn switch_refuses_when_client_running() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".zcode/v2")).unwrap();
        std::fs::write(
            dir.path().join(".zcode/v2/credentials.json"),
            b"{\"zcodejwttoken\":\"old\"}",
        )
        .unwrap();

        let provider = ZCodeProvider::at(dir.path().to_path_buf(), None).assume_alive(true);
        let mut cred = Credential::default();
        cred.put("credentials", creds("42"));

        let err = switch_to(&provider, "42", "慎静", &cred).unwrap_err();
        assert_eq!(err.kind, crate::app::error::ErrorKind::ClientRunning);
        // 客户端在跑时一个字节都不能写
        let kept = std::fs::read(dir.path().join(".zcode/v2/credentials.json")).unwrap();
        assert_eq!(kept, b"{\"zcodejwttoken\":\"old\"}");
    }

    #[test]
    fn switch_refuses_without_credentials() {
        let dir = tempfile::TempDir::new().unwrap();
        let provider = ZCodeProvider::at(dir.path().to_path_buf(), None);
        let err = switch_to(&provider, "1", "n", &Credential::default()).unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Invalid));
    }
}
