//! Qoder 账号扫描：读客户端的 Chromium 信封登录态。
//!
//! 两个版本共用一套逻辑，只有 profile 目录不同：
//!   - 国内版 `%APPDATA%\com.qodercn.app.stable`
//!   - 国际版 `%APPDATA%\com.qoder.app.stable`
//!
//! 文件结构：`Local State` 里的 `os_crypt.encrypted_key`（DPAPI 保护的主密钥）
//! + `auth.v1.dat`（`v10` 信封的 AES-256-GCM 密文）。解出来的 JSON 里有
//! `token` / `refreshToken` / `expiresAt` / `user{id,name,email,phone}`。

use crate::app::accounts::AccountIdentity;
use crate::app::error::{AppError, Result};
use crate::app::secret;
use std::path::PathBuf;

pub struct QoderProvider {
    pub intl: bool,
}

/// 扫描产物：身份信息 + 明文凭据。
/// **凭据必须在回调里立刻加密入库，不要收集成集合**——那样内存里会同时存在多个 token，
/// 正是 v1「整包解密」的毛病。
pub struct ScannedAccount {
    pub identity: AccountIdentity,
    pub secret: Vec<u8>,
}

impl QoderProvider {
    pub fn new(intl: bool) -> Self {
        Self { intl }
    }

    pub fn platform(&self) -> &'static str {
        if self.intl {
            "qoder_intl"
        } else {
            "qoder"
        }
    }

    pub fn label(&self) -> &'static str {
        if self.intl {
            "Qoder 国际版"
        } else {
            "Qoder CN"
        }
    }

    pub fn profile_dir(&self) -> Option<PathBuf> {
        let appdata = std::env::var("APPDATA").ok()?;
        let name = if self.intl { "com.qoder.app.stable" } else { "com.qodercn.app.stable" };
        Some(PathBuf::from(appdata).join(name))
    }

    /// 扫描并逐个交出账号。返回扫到的条数。
    pub fn scan(&self, mut on_account: impl FnMut(ScannedAccount) -> Result<()>) -> Result<usize> {
        let Some(profile) = self.profile_dir() else {
            return Ok(0);
        };
        let auth_file = profile.join("auth.v1.dat");
        let local_state = profile.join("Local State");
        if !auth_file.exists() || !local_state.exists() {
            return Ok(0);
        }

        let key = secret::chromium_key(&local_state)?;
        let blob = std::fs::read(&auth_file)?;
        let plain = secret::chromium_open(&key, &blob)?;

        let auth: serde_json::Value = serde_json::from_slice(&plain)
            .map_err(|e| AppError::new("invalid", format!("登录态不是 JSON：{e}")))?;

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
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_else(|| token.chars().rev().take(16).collect::<String>().chars().rev().collect());

        let identity = AccountIdentity {
            iid: format!("{}:{account_id}", self.platform()),
            platform: self.platform().to_string(),
            platform_label: self.label().to_string(),
            account_id,
            name,
            expires_at: auth.get("expiresAt").and_then(crate::app::timeutil::parse_ts),
            // 只留文件名：绝对路径里有用户名，属于不该出界的隐私
            source: "auth.v1.dat".to_string(),
            requires_checkin: true,
            has_secret: true,
        };

        // 凭据包只保留后续要用到的字段，其余（如 cookie）不落库
        let bundle = serde_json::json!({
            "token": token,
            "refresh_token": auth.get("refreshToken"),
            "refresh_expires_at": auth.get("refreshTokenExpiresAt"),
            "email": user.get("email"),
            "phone": user.get("phone"),
            "uid": user.get("id"),
        });

        on_account(ScannedAccount { identity, secret: serde_json::to_vec(&bundle)? })?;
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实机验证：本机装有 Qoder 才跑（`cargo test --lib qoder -- --ignored --nocapture`）
    #[test]
    #[ignore]
    fn scans_real_qoder_login_state() {
        for intl in [false, true] {
            let provider = QoderProvider::new(intl);
            let mut found = Vec::new();
            let count = provider
                .scan(|acc| {
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

    #[test]
    fn missing_profile_returns_zero_not_error() {
        // 环境变量指向不存在的目录时，应视为「没装这个客户端」，不是错误
        let provider = QoderProvider { intl: false };
        if provider.profile_dir().map(|p| p.exists()).unwrap_or(false) {
            return; // 本机装了，跳过这条
        }
        assert_eq!(provider.scan(|_| Ok(())).unwrap(), 0);
    }
}
