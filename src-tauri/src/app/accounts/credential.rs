//! 凭据包：账号库里**唯一**以密文形式存在的东西。
//!
//! 字段名与 v1 的账号记录（`to_item()`）刻意保持一致——P1 导入的历史账号
//! 直接把整条记录加密存进来了，切换登录要读的 `token` / `refresh_token` /
//! `meta` 就在里面。P2 新扫到的账号也写同一套结构，于是「导入的」和「扫到的」
//! 账号走同一条读写路径，不需要按来源分支。
//!
//! **边界**：`Credential` 只在 `repo::secret_of()` 之后短暂存在，
//! 用于一次切换或一次接口调用；不得挂到 `AccountView` 上、不得进日志。

use crate::app::error::{AppError, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Credential {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// ISO8601；v1 用空串表示「不知道」，这里保持同一口径
    #[serde(default)]
    pub expires_at: String,
    /// 平台相关字段，键名沿用 v1：uid / edition / domain / email / phone /
    /// region / device_id / credentials / config / ticket …
    #[serde(default)]
    pub meta: Map<String, Value>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 手写 Debug：token / meta 里是登录态与账号信息，
        // 派生实现会让任何一次 {:?}（unwrap_err 的 panic 消息也算）带出明文
        f.debug_struct("Credential")
            .field("token", &format!("[redacted:{}]", self.token.len()))
            .field(
                "refresh_token",
                &format!("[redacted:{}]", self.refresh_token.len()),
            )
            .field("expires_at", &self.expires_at)
            .field("meta_keys", &self.meta.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Credential {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes)
            .map_err(|e| AppError::new(ErrorKind::Invalid, format!("凭据包不是合法 JSON：{e}")))
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// v1 把 refresh_token 写在顶层，个别路径又复制进 meta；两处都认。
    pub fn refresh(&self) -> &str {
        if !self.refresh_token.is_empty() {
            return &self.refresh_token;
        }
        self.meta_str("refresh_token").unwrap_or("")
    }

    pub fn expires_ms(&self) -> Option<i64> {
        let text = self.expires_at.trim();
        if text.is_empty() {
            return None;
        }
        crate::app::timeutil::parse_ts(&Value::String(text.to_string()))
    }

    pub fn meta_str(&self, key: &str) -> Option<&str> {
        self.meta.get(key).and_then(|v| v.as_str())
    }

    pub fn meta_json(&self, key: &str) -> Option<&Value> {
        self.meta.get(key).filter(|v| !v.is_null())
    }

    pub fn put(&mut self, key: &str, value: impl Into<Value>) {
        self.meta.insert(key.to_string(), value.into());
    }
}

/// unix 毫秒 → ISO8601（UTC）。v1 的字段就是 ISO，写回时不能变成数字。
pub fn iso_from_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// P1 实际导入进来的形状（v1 `to_item()`），一个字都不能改
    const V1_ITEM: &str = r#"{
        "platform": "workbuddy",
        "account_id": "cceec57c-ac71-48e0-996c-6df5cc602cf3",
        "name": "凌致",
        "token": "tk_live",
        "refresh_token": "rt_live",
        "expires_at": "2026-11-20T10:00:00+00:00",
        "requires_checkin": true,
        "meta": {"uid": "cceec57c-ac71-48e0-996c-6df5cc602cf3", "domain": "www.workbuddy.cn"}
    }"#;

    #[test]
    fn reads_v1_imported_record() {
        let cred = Credential::parse(V1_ITEM.as_bytes()).unwrap();
        assert_eq!(cred.token, "tk_live");
        assert_eq!(cred.refresh(), "rt_live");
        assert_eq!(
            cred.meta_str("uid"),
            Some("cceec57c-ac71-48e0-996c-6df5cc602cf3")
        );
        assert!(cred.expires_ms().is_some(), "ISO 到期时间必须能解析成 ms");
    }

    #[test]
    fn roundtrip_keeps_every_field() {
        let cred = Credential::parse(V1_ITEM.as_bytes()).unwrap();
        let again = Credential::parse(&cred.to_bytes().unwrap()).unwrap();
        assert_eq!(again.token, cred.token);
        assert_eq!(again.meta, cred.meta);
    }

    #[test]
    fn refresh_falls_back_to_meta() {
        // 有些平台把 refresh 只写在 meta 里（v1 as_accounts 会两边都塞）
        let cred = Credential::parse(br#"{"token":"t","meta":{"refresh_token":"rt"}}"#).unwrap();
        assert_eq!(cred.refresh(), "rt");
    }

    #[test]
    fn empty_expiry_is_unknown_not_epoch() {
        let cred = Credential::parse(br#"{"token":"t","expires_at":""}"#).unwrap();
        assert_eq!(cred.expires_ms(), None);
    }

    #[test]
    fn bad_json_is_an_invalid_error() {
        let err = Credential::parse(b"not json").unwrap_err();
        assert_eq!(err.kind, crate::app::error::ErrorKind::Invalid);
    }

    #[test]
    fn iso_roundtrip_is_utc_seconds() {
        let iso = iso_from_ms(1_700_000_000_000);
        assert_eq!(iso, "2023-11-14T22:13:20Z");
        assert_eq!(
            Credential {
                expires_at: iso,
                ..Default::default()
            }
            .expires_ms(),
            Some(1_700_000_000_000)
        );
    }

    #[test]
    fn put_stores_platform_specific_values() {
        let mut cred = Credential::default();
        cred.put("region", json!("cn"));
        assert_eq!(cred.meta_str("region"), Some("cn"));
    }
}
