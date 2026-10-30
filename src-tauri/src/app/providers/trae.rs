//! Trae CN / TRAE SOLO CN：iCube 登录态（`storage.json` 里的 base64 密文）。
//!
//! 格式（v1 已逆向并稳定使用）：`base64(6 字节头 + 32 字节密钥 + AES-128-CBC 密文)`，
//! 解密密钥由那 32 字节密钥经两层 SHA-512 与两张固定置换表推导，
//! 明文前 64 字节是 HMAC（不校验），之后才是 JSON。
//!
//! **只能读，不能写**：这段登录态与设备绑定，重写会让客户端判定环境异常。

use super::{Provider, ScannedAccount, Sink};
use crate::app::accounts::credential::{iso_from_ms, Credential};
use crate::app::accounts::AccountIdentity;
use crate::app::error::{AppError, Result};
use base64::Engine;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use cbc::Decryptor;
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::path::PathBuf;

const AUTH_KEY: &str = "iCubeAuthInfo://icube.cloudide";
const HDR_LEN: usize = 6;
const KEY_LEN: usize = 32;
const HMAC_LEN: usize = 64;
const CLIENT_DIRS: [&str; 2] = ["Trae CN", "TRAE SOLO CN"];

#[rustfmt::skip]
const URE: [u8; 64] = [
    82, 9, 106, 213, 48, 54, 165, 56, 191, 64, 163, 158, 129, 243, 215, 251, 124, 227, 57, 130,
    155, 47, 255, 135, 52, 142, 67, 68, 196, 222, 233, 203, 84, 123, 148, 50, 166, 194, 35, 61,
    238, 76, 149, 11, 66, 250, 195, 78, 8, 46, 161, 102, 40, 217, 36, 178, 118, 91, 162, 73,
    109, 139, 209, 37,
];
#[rustfmt::skip]
const DRE: [u8; 64] = [
    31, 221, 168, 51, 136, 7, 199, 49, 177, 18, 16, 89, 39, 128, 236, 95, 96, 81, 127, 169, 25,
    181, 74, 13, 45, 229, 122, 159, 147, 201, 156, 239, 160, 224, 59, 77, 174, 42, 245, 176, 200,
    235, 187, 60, 131, 83, 153, 97, 23, 43, 4, 126, 186, 119, 214, 38, 225, 105, 20, 99, 85, 33,
    12, 125,
];

/// 解开 iCube 登录态。
pub fn decrypt_auth(b64_text: &str) -> Result<Value> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64_text.trim().as_bytes())
        .map_err(|e| AppError::new("invalid", format!("iCube 登录态不是合法 base64：{e}")))?;
    if raw.len() <= HDR_LEN + KEY_LEN {
        return Err(AppError::new("invalid", "iCube 登录态长度不足"));
    }
    let key = &raw[HDR_LEN..HDR_LEN + KEY_LEN];
    let mut seed_input = Vec::with_capacity(64 + 64);
    seed_input.extend_from_slice(&Sha512::digest(key));
    seed_input.extend(URE.iter().zip(DRE.iter()).map(|(a, b)| a ^ b));
    let seed = Sha512::digest(&seed_input);

    let mut buf = raw[HDR_LEN + KEY_LEN..].to_vec();
    let plain = Decryptor::<aes_gcm::aes::Aes128>::new_from_slices(&seed[..16], &seed[16..32])
        .map_err(|_| AppError::new("crypto", "iCube 解密参数非法"))?
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| AppError::new("crypto", "iCube 解密失败（填充不合法）"))?;
    if plain.len() <= HMAC_LEN {
        return Err(AppError::new("invalid", "iCube 明文长度不足"));
    }
    serde_json::from_slice(&plain[HMAC_LEN..])
        .map_err(|e| AppError::new("invalid", format!("iCube 明文不是 JSON：{e}")))
}

/// JWT 的 `exp`（秒）→ ms。Trae 的 `expiredAt` 有时缺失，只能看 token。
pub fn jwt_expiry_ms(token: &str) -> Option<i64> {
    let part = token.split('.').nth(1)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .ok()?;
    let data: Value = serde_json::from_slice(&raw).ok()?;
    data.get("exp").and_then(|v| v.as_i64()).map(|sec| sec * 1000)
}

/// `device_id`：先看 ahanet 配置，再看日志里的 `[ICDRS] ... did: 123`。
/// 没有正则依赖：两处都是「找到标记后跳过非字母数字，再取连续数字」。
fn device_id(app_dir: &std::path::Path) -> String {
    let config = app_dir.join("ahanet").join("tt_net_config.config");
    if let Ok(text) = std::fs::read_to_string(&config) {
        if let Some(id) = digits_after(&text, "device_id") {
            return id;
        }
    }
    let logs = app_dir.join("logs");
    // 客户端按日期分目录：logs/<yyyyMMdd>/main.log，也兼容直接放在 logs/ 下
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&logs).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.push(path.join("main.log"));
        } else if path.extension().and_then(|e| e.to_str()) == Some("log") {
            files.push(path);
        }
    }
    files.retain(|p| p.is_file());
    files.sort_by_key(|p| std::cmp::Reverse(p.metadata().and_then(|m| m.modified()).ok()));
    for path in files.into_iter().take(5) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(id) = text
                .find("[ICDRS]")
                .and_then(|at| text[at..].find("did: ").map(|off| at + off))
                .and_then(|at| digits_after(&text[at..], "did: "))
            {
                return id;
            }
        }
    }
    String::new()
}

/// 标记之后、跳过非字母数字后的连续数字。
fn digits_after(haystack: &str, marker: &str) -> Option<String> {
    let mut rest = &haystack[haystack.find(marker)? + marker.len()..];
    rest = rest.trim_start_matches(|c: char| !c.is_ascii_digit());
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

pub struct TraeProvider {
    /// `%APPDATA%`；测试可注入
    appdata: Option<PathBuf>,
}

impl TraeProvider {
    pub fn new() -> Self {
        Self { appdata: std::env::var("APPDATA").ok().map(PathBuf::from) }
    }

    pub fn at(appdata: PathBuf) -> Self {
        Self { appdata: Some(appdata) }
    }

    fn storage_files(&self) -> Vec<PathBuf> {
        let Some(base) = self.appdata.as_ref() else { return Vec::new() };
        CLIENT_DIRS
            .iter()
            .map(|dir| base.join(dir).join("User").join("globalStorage").join("storage.json"))
            .filter(|p| p.exists())
            .collect()
    }
}

impl Default for TraeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for TraeProvider {
    fn platform(&self) -> &'static str {
        "trae"
    }

    fn label(&self) -> &'static str {
        "Trae CN"
    }

    fn scan(&self, sink: &mut Sink<'_>) -> Result<usize> {
        let mut count = 0;
        let mut seen = std::collections::HashSet::new();
        for path in self.storage_files() {
            let Ok(raw) = std::fs::read(&path) else { continue };
            let Ok(storage) = serde_json::from_slice::<Value>(&raw) else { continue };
            let Some(encrypted) = storage.get(AUTH_KEY).and_then(|v| v.as_str()) else { continue };
            let Ok(auth) = decrypt_auth(encrypted) else { continue };
            let Some(token) = auth.get("token").and_then(|v| v.as_str()) else { continue };
            if !seen.insert(token.to_string()) {
                continue;
            }
            let account = auth.get("account").cloned().unwrap_or_default();
            let account_id = auth
                .get("userId")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| token.chars().rev().take(16).collect::<String>().chars().rev().collect());
            let name = account
                .get("username")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("用户{account_id}"));
            let expires_at = auth
                .get("expiredAt")
                .and_then(crate::app::timeutil::parse_ts)
                .or_else(|| jwt_expiry_ms(token));
            let region = auth
                .pointer("/userRegion/region")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let mut cred = Credential {
                token: token.to_string(),
                expires_at: expires_at.map(iso_from_ms).unwrap_or_default(),
                ..Default::default()
            };
            cred.put("uid", json!(account_id));
            cred.put("name", json!(name));
            cred.put("region", json!(region));
            // storage.json 在 <客户端目录>/User/globalStorage/ 下，往上三层才是客户端根目录
            cred.put(
                "device_id",
                json!(path.ancestors().nth(3).map(device_id).unwrap_or_default()),
            );

            let identity = AccountIdentity {
                iid: format!("trae:{account_id}"),
                platform: "trae".into(),
                platform_label: "Trae CN".into(),
                account_id,
                name,
                expires_at,
                source: "storage.json".into(),
                requires_checkin: true,
                has_secret: true,
            };
            sink(ScannedAccount::new(identity, &cred)?)?;
            count += 1;
        }
        Ok(count)
    }

    fn current_id(&self) -> Option<String> {
        for path in self.storage_files() {
            let Some(encrypted) = std::fs::read(&path)
                .ok()
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                .and_then(|storage| storage.get(AUTH_KEY).and_then(|v| v.as_str()).map(str::to_string))
            else {
                continue;
            };
            let Some(auth) = decrypt_auth(&encrypted).ok() else { continue };
            if let Some(id) = auth.get("userId") {
                return Some(match id {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::cipher::BlockEncryptMut;

    /// 按客户端格式造一份密文（等价于 v1 里从 storage.json 拿到的东西）
    fn seal(payload: &Value) -> String {
        let key = [7u8; KEY_LEN];
        let mut seed_input = Vec::new();
        seed_input.extend_from_slice(&Sha512::digest(key));
        seed_input.extend(URE.iter().zip(DRE.iter()).map(|(a, b)| a ^ b));
        let seed = Sha512::digest(&seed_input);

        let mut plain = vec![0u8; HMAC_LEN];
        plain.extend_from_slice(&serde_json::to_vec(payload).unwrap());

        let mut buf = plain.clone();
        buf.resize(plain.len() + 16, 0);
        let len = {
            let ciphertext = cbc::Encryptor::<aes_gcm::aes::Aes128>::new_from_slices(
                &seed[..16],
                &seed[16..32],
            )
            .unwrap()
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len())
            .unwrap();
            ciphertext.len()
        };
        buf.truncate(len);
        let ciphertext = buf;

        let mut raw = vec![0u8; HDR_LEN];
        raw.extend_from_slice(&key);
        raw.extend_from_slice(&ciphertext);
        base64::engine::general_purpose::STANDARD.encode(raw)
    }

    #[test]
    fn decrypts_icube_payload() {
        let payload = json!({
            "token": "tk",
            "userId": "u1",
            "account": {"username": "某人"},
            "expiredAt": 1_700_000_000_000i64,
            "userRegion": {"region": "cn"},
        });
        let auth = decrypt_auth(&seal(&payload)).unwrap();
        assert_eq!(auth.get("token").unwrap(), "tk");
        assert_eq!(auth.pointer("/account/username").unwrap(), "某人");
    }

    #[test]
    fn short_or_bad_input_is_an_error() {
        assert!(decrypt_auth("!!!not-base64!!!").is_err());
        let short = base64::engine::general_purpose::STANDARD.encode([0u8; 10]);
        assert!(decrypt_auth(&short).is_err());
    }

    #[test]
    fn jwt_expiry_reads_exp_in_seconds() {
        // {"exp":1700000000}
        let token = "h.eyJleHAiOjE3MDAwMDAwMDB9.s";
        assert_eq!(jwt_expiry_ms(token), Some(1_700_000_000_000));
        assert_eq!(jwt_expiry_ms("not-a-jwt"), None);
    }

    #[test]
    fn device_id_prefers_ahanet_config() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("ahanet")).unwrap();
        std::fs::write(
            dir.path().join("ahanet/tt_net_config.config"),
            "{\"device_id\": 123456789}",
        )
        .unwrap();
        assert_eq!(device_id(dir.path()), "123456789");
    }

    #[test]
    fn device_id_falls_back_to_logs() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("logs/20261030")).unwrap();
        std::fs::write(
            dir.path().join("logs/20261030/main.log"),
            "[ICDRS] init ok did: 987654321 end",
        )
        .unwrap();
        assert_eq!(device_id(dir.path()), "987654321");
    }

    #[test]
    fn missing_client_returns_zero() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(TraeProvider::at(dir.path().to_path_buf()).scan(&mut |_| Ok(())).unwrap(), 0);
    }

    #[test]
    fn scan_reads_storage_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let storage_dir = dir.path().join("Trae CN/User/globalStorage");
        std::fs::create_dir_all(&storage_dir).unwrap();
        let payload = json!({
            "token": "tk",
            "userId": "u1",
            "account": {"username": "某人"},
            "expiredAt": 1_700_000_000_000i64,
        });
        std::fs::write(
            storage_dir.join("storage.json"),
            serde_json::to_vec(&json!({ AUTH_KEY: seal(&payload) })).unwrap(),
        )
        .unwrap();

        let mut rows = Vec::new();
        TraeProvider::at(dir.path().to_path_buf())
            .scan(&mut |acc| { rows.push(acc); Ok(()) })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].identity.iid, "trae:u1");
        assert_eq!(rows[0].identity.name, "某人");
    }
}
