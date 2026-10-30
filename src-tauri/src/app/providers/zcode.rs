//! ZCode：登录态就是一份 JSON（`credentials.json` + `config.json`），整份存、整份写回。
//!
//! 字段值带 `enc:v1:` 前缀，密钥由「平台 + 家目录 + 用户名」推导（不落盘、不随账号变）。
//! 因此**原始密文可以原样搬运**：换账号只需要把那两份 JSON 写回去，
//! 不需要解密再重新加密——这也避免了「重新加密后密钥不一致」这类问题。
//!
//! 设备身份（telemetry 的 `deviceMid` 与 `rum-electron-store` 里的 `_arms_uid`）
//! 跟着账号走：不同账号共用同一个设备号会被服务端算成同一个人，额度就串了。

use super::{Provider, ScannedAccount, Sink, SwitchOutcome};
use crate::app::accounts::credential::{iso_from_ms, Credential};
use crate::app::accounts::AccountIdentity;
use crate::app::error::{AppError, Result};
use crate::app::{fsx, paths, process};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key};
use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const PREFIX: &str = "enc:v1:";

pub struct ZCodeProvider {
    /// ZCode 的 dataBaseDir（`~/.zcode` 的父目录）
    data_root: PathBuf,
    /// `%APPDATA%`，设备指纹在它下面的 ZCode 目录里
    appdata: Option<PathBuf>,
    /// 客户端全局设置。固定在家目录，**不跟随 dataBaseDir**（v1 的实测结论）
    setting: PathBuf,
}

impl ZCodeProvider {
    pub fn new() -> Self {
        Self {
            data_root: default_data_root(),
            appdata: std::env::var("APPDATA").ok().map(PathBuf::from),
            setting: paths::home().join(".zcode").join("v2").join("setting.json"),
        }
    }

    pub fn at(data_root: PathBuf, appdata: Option<PathBuf>) -> Self {
        let setting = data_root.join(".zcode").join("v2").join("setting.json");
        Self { data_root, appdata, setting }
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
        data.get("deviceMid").and_then(|v| v.as_str()).map(str::to_string)
    }

    fn arms_files(&self) -> Vec<PathBuf> {
        let Some(appdata) = self.appdata.as_ref().filter(|p| p.is_dir()) else { return Vec::new() };
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
            inner.sort_by_key(|p| p.file_name().map(|n| n != "ZGVmYXVsdA.json").unwrap_or(true));
            files.extend(inner);
        }
        files
    }

    fn arms_uid(&self) -> Option<String> {
        self.arms_files().into_iter().find_map(|path| {
            let data = Self::read_json(&path)?;
            data.get("_arms_uid").and_then(|v| v.as_str()).map(str::to_string)
        })
    }
}

impl Default for ZCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------------ 加解密

/// 客户端推导密钥的口径：`zcode-credential-fallback:win32:{家目录}:{用户名}`。
/// 不能换成 DPAPI——那是本工具自己的账号库在用，与 ZCode 无关。
fn secret() -> String {
    if let Ok(env) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        if !env.is_empty() {
            return env;
        }
    }
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "unknown".into());
    format!("zcode-credential-fallback:win32:{}:{user}", paths::home().display())
}

fn key() -> Key<Aes256Gcm> {
    let digest = Sha256::digest(secret().as_bytes());
    *Key::<Aes256Gcm>::from_slice(&digest)
}

fn b64(text: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|e| AppError::new("invalid", format!("不是合法 base64url：{e}")))
}

/// 解开一个 `enc:v1:` 字段；非加密值原样返回。
pub fn decrypt(value: &str) -> Option<String> {
    let Some(body) = value.strip_prefix(PREFIX) else { return Some(value.to_string()) };
    let parts: Vec<&str> = body.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let nonce = b64(parts[0]).ok()?;
    let tag = b64(parts[1]).ok()?;
    let mut cipher = b64(parts[2]).ok()?;
    cipher.extend_from_slice(&tag);
    let plain = Aes256Gcm::new(&key())
        .decrypt(aes_gcm::Nonce::from_slice(&nonce), cipher.as_ref())
        .ok()?;
    String::from_utf8(plain).ok()
}

fn field<'a>(creds: &'a Value, key: &str) -> Option<String> {
    decrypt(creds.get(key)?.as_str()?)
}

fn json_field(creds: &Value, key: &str) -> Option<Value> {
    serde_json::from_str(&field(creds, key)?).ok()
}

fn jwt_payload(token: &str) -> Map<String, Value> {
    let Some(part) = token.split('.').nth(1) else { return Map::new() };
    b64(part)
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn is_logged_in(creds: &Value) -> bool {
    let Some(obj) = creds.as_object() else { return false };
    let oauth = obj.keys().any(|k| k.starts_with("oauth:") && k.ends_with(":access_token"));
    let token = obj.get("zcodejwttoken").and_then(|v| v.as_str()).unwrap_or("").trim();
    oauth || !token.is_empty()
}

/// 账号身份。返回 (account_id, 昵称, 到期 ms, provider, email)
pub fn identity(creds: &Value) -> (String, String, Option<i64>, String, String) {
    let provider = field(creds, "oauth:active_provider").unwrap_or_else(|| "bigmodel".into());
    let info = json_field(creds, &format!("oauth:{provider}:user_info")).unwrap_or(Value::Null);
    let mut user_id = info.get("id").and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    if user_id.is_none() {
        let access = field(creds, &format!("oauth:{provider}:access_token")).unwrap_or_default();
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
        .or_else(|| info.get("username").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .map(str::to_string)
        .unwrap_or_else(|| {
            // 手机号登录的账号，客户端把手机号写成 xxx@phone.local，只留号码
            email.split('@').next().unwrap_or("").to_string()
        })
        .trim()
        .to_string();
    let name = if name.is_empty() { "ZCode 账号".to_string() } else { name };

    let expires = field(creds, "zcodejwttoken")
        .and_then(|token| jwt_payload(&token).get("exp").and_then(|v| v.as_i64()))
        .map(|sec| sec * 1000);
    let account_id = if !user_id.as_deref().unwrap_or("").is_empty() {
        user_id.unwrap()
    } else if !email.is_empty() {
        email.clone()
    } else if !name.is_empty() {
        name.clone()
    } else {
        "unknown".to_string()
    };
    (account_id, name, expires, provider, email)
}

impl Provider for ZCodeProvider {
    fn platform(&self) -> &'static str {
        "zcode"
    }

    fn label(&self) -> &'static str {
        "ZCode"
    }

    fn scan(&self, sink: &mut Sink<'_>) -> Result<usize> {
        let Some(creds) = self.live_credentials() else { return Ok(0) };
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
        if let Some(mid) = self.device_mid() {
            cred.put("device_mid", json!(mid));
        }
        if let Some(arms) = self.arms_uid() {
            cred.put("arms_uid", json!(arms));
        }

        let identity = AccountIdentity {
            iid: format!("zcode:{account_id}"),
            platform: "zcode".into(),
            platform_label: "ZCode".into(),
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
}

// ------------------------------------------------------------------ 切换

/// 把一份档案写回客户端。调用方负责先确认客户端已关闭。
pub fn switch_to(
    provider: &ZCodeProvider,
    account_id: &str,
    name: &str,
    cred: &Credential,
    backup_dir: &Path,
) -> Result<SwitchOutcome> {
    let Some(creds) = cred.meta_json("credentials").filter(|v| v.is_object()) else {
        return Err(AppError::new("invalid", "这条账号里没有 ZCode 登录凭据，无法切换"));
    };
    if !is_logged_in(creds) {
        return Err(AppError::new("invalid", "登录凭据已经失效，请重新登录一次该账号"));
    }
    if provider.current_id().as_deref() == Some(account_id) {
        return Err(AppError::new("invalid", "这个账号就是客户端当前登录的账号，不需要切换"));
    }
    if process::process_running(&["ZCode.exe", "zcode.exe"]) {
        return Err(AppError::new("client_running", "请先关闭 ZCode 客户端再切换"));
    }

    let target = provider.credentials_path();
    let mut backup = None;
    if target.exists() {
        backup = Some(fsx::backup_to(&target, backup_dir, "zcode")?);
    }
    // 写的是原始密文，不是解密后的明文：密钥由机器推导，重新加密反而可能与客户端不一致
    fsx::write_atomic(&target, &serde_json::to_vec_pretty(creds)?)?;

    if let Some(config) = cred.meta_json("config").filter(|v| v.is_object()) {
        let path = provider.config_path();
        if path.exists() {
            let _ = fsx::backup_to(&path, backup_dir, "zcode-config");
        }
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
    let _ = std::fs::remove_file(provider.plan_cache_path());
    align_family_domain(provider, creds);

    Ok(SwitchOutcome { message: format!("ZCode 已切换到 {name}"), backup })
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
    let Some(active) = field(creds, "oauth:active_provider") else { return };
    if active != "bigmodel" && active != "zai" {
        return;
    }
    let path = provider.setting_path();
    let Some(mut data) = ZCodeProvider::read_json(&path) else { return };
    let Some(obj) = data.as_object_mut() else { return };
    obj.insert("providerFamilyDomain".into(), json!(active));
    obj.insert("providerFamilyDomainUpdatedAt".into(), json!(crate::app::timeutil::now_ms()));
    let _ = fsx::write_atomic(&path, &serde_json::to_vec_pretty(&data).unwrap_or_default());
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
        .and_then(|v| v.get("dataBaseDir").and_then(|v| v.as_str()).map(PathBuf::from))
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
        let sealed = Aes256Gcm::new(&key()).encrypt(aes_gcm::Nonce::from_slice(&nonce), value.as_bytes()).unwrap();
        let (cipher, tag) = sealed.split_at(sealed.len() - 16);
        format!(
            "{PREFIX}{}.{}.{}",
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
        assert_eq!(name, "17679130626", "昵称是手机号；脱敏在视图层做，这里保留原值");
    }

    #[test]
    fn scan_reads_credentials_and_keeps_ciphertext() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".zcode/v2")).unwrap();
        std::fs::write(dir.path().join(".zcode/v2/credentials.json"), serde_json::to_vec(&creds("9")).unwrap()).unwrap();

        let provider = ZCodeProvider::at(dir.path().to_path_buf(), None);
        let mut rows = Vec::new();
        provider.scan(&mut |acc| { rows.push(acc); Ok(()) }).unwrap();

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
        std::fs::write(dir.path().join(".zcode/v2/credentials.json"), b"{\"zcodejwttoken\":\"old\"}").unwrap();

        let provider = ZCodeProvider::at(dir.path().to_path_buf(), None);
        let mut cred = Credential::default();
        cred.put("credentials", creds("42"));
        cred.put("config", json!({"theme": "dark"}));

        let out = switch_to(&provider, "42", "慎静", &cred, &dir.path().join("bk")).unwrap();
        assert!(out.backup.is_some());
        let written: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(".zcode/v2/credentials.json")).unwrap()).unwrap();
        assert_eq!(identity(&written).0, "42");
        assert!(dir.path().join(".zcode/v2/config.json").exists());
    }

    #[test]
    fn switch_refuses_without_credentials() {
        let dir = tempfile::TempDir::new().unwrap();
        let provider = ZCodeProvider::at(dir.path().to_path_buf(), None);
        let err = switch_to(&provider, "1", "n", &Credential::default(), dir.path()).unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Invalid));
    }
}
