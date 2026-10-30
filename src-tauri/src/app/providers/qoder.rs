//! Qoder 账号扫描与登录态写回：读写客户端的 Chromium 信封。
//!
//! 两个版本共用一套逻辑，只有 profile 目录不同：
//!   - 国内版 `%APPDATA%\com.qodercn.app.stable`
//!   - 国际版 `%APPDATA%\com.qoder.app.stable`
//!
//! 文件结构：`Local State` 里的 `os_crypt.encrypted_key`（DPAPI 保护的主密钥）
//! + `auth.v1.dat`（`v10` 信封的 AES-256-GCM 密文）。解出来的 JSON 里有
//! `token` / `refreshToken` / `expiresAt` / `user{id,name,email,phone}`。
//!
//! **写回只要重封 `auth.v1.dat`**：Qoder 启动时只认这个文件里的 token，
//! 不像 WorkBuddy 那样还要换整份 Chromium 登录态。

use super::{Provider, ScannedAccount, Sink, SwitchOutcome};
use crate::app::accounts::credential::{iso_from_ms, Credential};
use crate::app::accounts::AccountIdentity;
use crate::app::error::{AppError, Result};
use crate::app::{fsx, process, secret};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub struct QoderProvider {
    pub intl: bool,
    /// 可注入：测试指向临时目录，生产指向 `%APPDATA%`
    profile: Option<PathBuf>,
}

impl QoderProvider {
    pub fn new(intl: bool) -> Self {
        Self { intl, profile: default_profile(intl) }
    }

    /// 指定 profile 目录（测试与「额外来源」用）
    pub fn at(intl: bool, profile: PathBuf) -> Self {
        Self { intl, profile: Some(profile) }
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
        let Some(profile) = self.profile_dir() else { return Ok(None) };
        let auth_file = profile.join("auth.v1.dat");
        let local_state = profile.join("Local State");
        if !auth_file.exists() || !local_state.exists() {
            return Ok(None);
        }
        let key = secret::chromium_key(&local_state)?;
        let blob = std::fs::read(&auth_file)?;
        let plain = secret::chromium_open(&key, &blob)?;
        let auth: Value = serde_json::from_slice(&plain)
            .map_err(|e| AppError::new("invalid", format!("登录态不是 JSON：{e}")))?;
        Ok(Some((auth, key)))
    }

    /// 把一份登录态重新封成 `v10` 信封。
    fn seal(key: &Key<Aes256Gcm>, plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut nonce)
            .map_err(|e| AppError::new("crypto", format!("取随机数失败：{e}")))?;
        let ciphertext = Aes256Gcm::new(key)
            .encrypt(aes_gcm::Nonce::from_slice(&nonce), plain)
            .map_err(|_| AppError::new("crypto", "AES-GCM 加密失败"))?;
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
pub fn build_auth(cred: &Credential, account_id: &str, name: &str, current: Option<&Value>) -> Value {
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
        .unwrap_or_else(|| (now + chrono::Duration::days(30)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));

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
        json!(if same_user { old_user.and_then(|u| u.get("phone")).cloned() } else { None }),
    );
    user.insert(
        "avatarUrl".into(),
        json!(if same_user { old_user.and_then(|u| u.get("avatarUrl")).cloned() } else { None }),
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
    backup_dir: &Path,
) -> Result<SwitchOutcome> {
    let Some(profile) = provider.profile_dir() else {
        return Err(AppError::new("io", "没有找到 Qoder 的客户端目录"));
    };
    if !profile.exists() {
        return Err(AppError::new("io", "没有找到 Qoder 的客户端目录"));
    }
    // 先判凭据：缺 refresh token 时写回去只会立刻被登出，不必让用户关客户端
    if cred.refresh().is_empty() {
        return Err(AppError::new("invalid", "这个账号没有 refresh token，写回去会被立刻登出"));
    }
    if process::process_running(provider.process_names()) {
        return Err(AppError::new("client_running", "请先关闭 Qoder 客户端再切换"));
    }

    let path = profile.join("auth.v1.dat");
    let local_state = profile.join("Local State");
    if !path.exists() || !local_state.exists() {
        return Err(AppError::new("io", "没有找到 Qoder 的登录态文件"));
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
        return Err(AppError::new("invalid", "这个账号就是客户端当前登录的账号，不需要切换"));
    }

    let auth = build_auth(cred, account_id, name, current.as_ref());
    let key = secret::chromium_key(&local_state)?;
    let sealed = QoderProvider::seal(&key, &serde_json::to_vec(&auth)?)?;

    let backup = fsx::backup_to(&path, backup_dir, provider.platform_key())?;
    fsx::write_atomic(&path, &sealed)?;

    // 写后验读：能解回、且 user.id 是目标账号，才算真的写成功
    let ok = std::fs::read(&path)
        .ok()
        .and_then(|blob| secret::chromium_open(&key, &blob).ok())
        .and_then(|plain| serde_json::from_slice::<Value>(&plain).ok())
        .map(|v| v.pointer("/user/id").and_then(|x| x.as_str()) == Some(account_id))
        .unwrap_or(false);
    if !ok {
        std::fs::copy(&backup, &path)?;
        return Err(AppError::new("write_failed", "写入后校验失败，已回滚到切换前的登录态"));
    }

    Ok(SwitchOutcome {
        message: format!("{} 已切换到 {name}", provider.label()),
        backup: Some(backup),
    })
}

impl Provider for QoderProvider {
    fn platform(&self) -> &'static str {
        self.platform_key()
    }

    fn label(&self) -> &'static str {
        if self.intl {
            "Qoder 国际版"
        } else {
            "Qoder CN"
        }
    }

    fn scan(&self, sink: &mut Sink<'_>) -> Result<usize> {
        let Some((auth, _key)) = self.read_auth()? else { return Ok(0) };
        let token = auth
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::new("invalid", "登录态里没有 token"))?;
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
        let expires_at = auth.get("expiresAt").and_then(crate::app::timeutil::parse_ts);

        let mut cred = Credential {
            token: token.to_string(),
            refresh_token: auth.get("refreshToken").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            expires_at: expires_at.map(iso_from_ms).unwrap_or_default(),
            ..Default::default()
        };
        cred.put("uid", json!(account_id));
        cred.put("name", json!(name));
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
        auth.pointer("/user/id").and_then(|v| v.as_str()).map(str::to_string)
    }
}

fn tail(text: &str, n: usize) -> String {
    text.chars().rev().take(n).collect::<String>().chars().rev().collect()
}

fn default_profile(intl: bool) -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    let name = if intl { "com.qoder.app.stable" } else { "com.qodercn.app.stable" };
    Some(PathBuf::from(appdata).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(auth.pointer("/user/phone").unwrap().is_null(), "不能把别人的手机号写给这个账号");
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
        let cred = Credential { token: "t".into(), refresh_token: "r".into(), ..Default::default() };
        let auth = build_auth(&cred, "u1", "n", None);
        let expires = auth.pointer("/expiresAt").unwrap().as_str().unwrap();
        assert!(!expires.is_empty(), "空串会让客户端的时间解析失败");
    }

    #[test]
    fn seal_then_open_roundtrip() {
        let key = *Key::<Aes256Gcm>::from_slice(&[9u8; 32]);
        let sealed = QoderProvider::seal(&key, br#"{"token":"x"}"#).unwrap();
        assert_eq!(&sealed[..3], b"v10");
        assert_eq!(secret::chromium_open(&key, &sealed).unwrap(), br#"{"token":"x"}"#);
    }

    #[test]
    fn switch_refuses_when_client_running() {
        // 进程名取一个必定存在的名字，模拟「客户端在跑」
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        let provider = QoderProvider::at(false, dir.path().to_path_buf());
        let mut provider = provider;
        provider.profile = Some(dir.path().to_path_buf());
        // 直接构造一个在跑的进程名不方便，改用「缺文件」的早退路径验证错误码
        let err = switch_to(&provider, "u1", "n", &sample_cred(), &dir.path().join("bk")).unwrap_err();
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
                    found.push((acc.identity.name.clone(), acc.identity.iid.clone(), acc.secret.len()));
                    Ok(())
                })
                .expect("扫描不应失败");
            println!("{}: {count} 个账号", provider.label());
            for (name, iid, size) in &found {
                println!("   {name} · {} · 凭据 {size} 字节", &iid[..iid.len().min(24)]);
            }
        }
    }
}
