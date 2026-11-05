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
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::types::{trim_f64, ActionResult, CreditLot, CreditSnapshot, StatusResult};
use async_trait::async_trait;
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
        .map_err(|e| {
            AppError::new(
                ErrorKind::Invalid,
                format!("iCube 登录态不是合法 base64：{e}"),
            )
        })?;
    if raw.len() <= HDR_LEN + KEY_LEN {
        return Err(AppError::new(ErrorKind::Invalid, "iCube 登录态长度不足"));
    }
    let key = &raw[HDR_LEN..HDR_LEN + KEY_LEN];
    let mut seed_input = Vec::with_capacity(64 + 64);
    seed_input.extend_from_slice(&Sha512::digest(key));
    seed_input.extend(URE.iter().zip(DRE.iter()).map(|(a, b)| a ^ b));
    let seed = Sha512::digest(&seed_input);

    let mut buf = raw[HDR_LEN + KEY_LEN..].to_vec();
    let plain = Decryptor::<aes_gcm::aes::Aes128>::new_from_slices(&seed[..16], &seed[16..32])
        .map_err(|_| AppError::new(ErrorKind::Crypto, "iCube 解密参数非法"))?
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| AppError::new(ErrorKind::Crypto, "iCube 解密失败（填充不合法）"))?;
    if plain.len() <= HMAC_LEN {
        return Err(AppError::new(ErrorKind::Invalid, "iCube 明文长度不足"));
    }
    serde_json::from_slice(&plain[HMAC_LEN..])
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("iCube 明文不是 JSON：{e}")))
}

/// JWT 的 `exp`（秒）→ ms。Trae 的 `expiredAt` 有时缺失，只能看 token。
pub fn jwt_expiry_ms(token: &str) -> Option<i64> {
    let part = token.split('.').nth(1)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .ok()?;
    let data: Value = serde_json::from_slice(&raw).ok()?;
    data.get("exp")
        .and_then(|v| v.as_i64())
        .map(|sec| sec * 1000)
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
        Self {
            appdata: std::env::var("APPDATA").ok().map(PathBuf::from),
        }
    }

    pub fn at(appdata: PathBuf) -> Self {
        Self {
            appdata: Some(appdata),
        }
    }

    fn storage_files(&self) -> Vec<PathBuf> {
        let Some(base) = self.appdata.as_ref() else {
            return Vec::new();
        };
        CLIENT_DIRS
            .iter()
            .map(|dir| {
                base.join(dir)
                    .join("User")
                    .join("globalStorage")
                    .join("storage.json")
            })
            .filter(|p| p.exists())
            .collect()
    }
}

impl Default for TraeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
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
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };
            let Ok(storage) = serde_json::from_slice::<Value>(&raw) else {
                continue;
            };
            let Some(encrypted) = storage.get(AUTH_KEY).and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(auth) = decrypt_auth(encrypted) else {
                continue;
            };
            let Some(token) = auth.get("token").and_then(|v| v.as_str()) else {
                continue;
            };
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
                .unwrap_or_else(|| {
                    token
                        .chars()
                        .rev()
                        .take(16)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect()
                });
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
                .and_then(|storage| {
                    storage
                        .get(AUTH_KEY)
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
            else {
                continue;
            };
            let Some(auth) = decrypt_auth(&encrypted).ok() else {
                continue;
            };
            if let Some(id) = auth.get("userId") {
                return Some(match id {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            }
        }
        None
    }

    // ------------------------------------------------------------ P3：签到与积分

    /// Trae 自带积分批次（谁在什么时候到账、什么时候到期），所以不写台账（契约 5）
    fn live_credit(&self) -> bool {
        true
    }

    async fn status(&self, cred: &Credential, http: &dyn Http) -> Result<StatusResult> {
        let data = unwrap(self.call(cred, http, "status").await?);
        if data.get("enable").and_then(|v| v.as_bool()) == Some(false) {
            return Ok(StatusResult {
                checked_in: None,
                summary: "签到未开放".into(),
                balance: None,
                lots: None,
                next_at: None,
            });
        }
        let checked = data.get("checked_in").and_then(|v| v.as_bool());
        // 用量接口失败不影响「今天签没签」，退回到签到接口自己带的积分数
        let (summary, balance, lots) = match self.usage(cred, http).await {
            Ok(triple) => triple,
            Err(_) => {
                let balance = data.get("credits").and_then(|v| v.as_f64());
                let summary = balance
                    .map(|b| format!("剩余 {} 积分", trim_f64(b)))
                    .unwrap_or_default();
                (summary, balance, None)
            }
        };
        Ok(StatusResult {
            checked_in: checked,
            summary,
            balance,
            lots,
            next_at: None,
        })
    }

    async fn credits(&self, cred: &Credential, http: &dyn Http) -> Result<CreditSnapshot> {
        // 只读套餐用量，不请求 checkin/status，因此不会查询签到状态
        let (summary, balance, lots) = self.usage(cred, http).await?;
        Ok(CreditSnapshot {
            summary,
            error: None,
            balance,
            lots,
            quota: None,
        })
    }

    async fn checkin(&self, cred: &Credential, http: &dyn Http) -> Result<ActionResult> {
        let state = self.status(cred, http).await?;
        if state.checked_in == Some(true) {
            return Ok(ActionResult {
                ok: true,
                message: format!("今日已签到（{}）", state.summary),
                granted: None,
                validity_days: None,
                already: true,
            });
        }
        let resp = self.call(cred, http, "claim").await?;
        if resp.get("code").and_then(|v| v.as_i64()) == Some(0) {
            // 领完之后再查一次，用余额差算出发放额（接口不直接给金额）
            let after = self.status(cred, http).await.ok();
            let granted = match (&state.balance, after.as_ref().and_then(|a| a.balance)) {
                (Some(before), Some(after)) => Some((after - before).max(0.0)).filter(|v| *v > 0.0),
                _ => None,
            };
            let summary = after.map(|a| a.summary).unwrap_or_default();
            return Ok(ActionResult {
                ok: true,
                message: if summary.is_empty() {
                    "签到成功".into()
                } else {
                    format!("签到成功，当前 {summary}")
                },
                granted,
                // 签到积分有效期 31 个自然日
                validity_days: Some(31),
                already: false,
            });
        }
        let message = resp
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("签到失败：{}", truncate(&resp.to_string(), 160)));
        Ok(ActionResult {
            ok: false,
            message,
            granted: None,
            validity_days: None,
            already: false,
        })
    }
}

const API_BASE: &str = "https://api.trae.cn/trae/api/v2/ug/checkin_credits";
const USAGE_URL: &str = "https://api.trae.cn/trae/api/v2/pay/ide_user_ent_usage";
// 带空格是刻意的：与 v1（requests 的 json.dumps 产物）逐字节一致，见 post() 注释
const REQ_SOURCE: &str = r#"{"req_source": 1}"#;

impl TraeProvider {
    /// 请求形态与 v1 逐字节对齐（2026-11-01 实测）：
    /// 同一 token、同一分钟，v2 的 `AI-Manager` UA + 紧凑 body 被 claim 拒
    /// （"The submitted order parameters are incorrect"），python-requests 的
    /// `python-requests/2.34.2` + `{"req_source": 1}` 成功。重登后签发的新 token
    /// 走更严的下单校验，UA/请求形态不在这个白名单里就拒单。别改这三处。
    fn post(&self, cred: &Credential, url: &str, body: &str) -> HttpRequest {
        let mut req = HttpRequest::post(url)
            .header("Authorization", format!("Cloud-IDE-JWT {}", cred.token))
            .header("Content-Type", "application/json")
            .header("User-Agent", "python-requests/2.34.2")
            .header("Accept", "*/*");
        if let Some(device) = cred.meta_str("device_id").filter(|s| !s.is_empty()) {
            req = req.header("x-device-id", device);
        }
        if let Some(region) = cred.meta_str("region").filter(|s| !s.is_empty()) {
            req = req.header("X-User-Region", region);
        }
        req.body = Some(body.to_string());
        req
    }

    async fn call(&self, cred: &Credential, http: &dyn Http, path: &str) -> Result<Value> {
        let url = format!("{API_BASE}/{path}");
        let resp = http.send(self.post(cred, &url, REQ_SOURCE)).await?;
        if resp.status >= 400 {
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("HTTP {}（登录态可能过期）", resp.status),
            ));
        }
        resp.json()
    }

    /// 套餐/积分包用量。按「先到期先消耗」还原每个批次还剩多少。
    async fn usage(
        &self,
        cred: &Credential,
        http: &dyn Http,
    ) -> Result<(String, Option<f64>, Option<Vec<CreditLot>>)> {
        let data = http
            .send(self.post(cred, USAGE_URL, REQ_SOURCE))
            .await?
            .json()?;
        if let Some(code) = data.get("code").and_then(|v| v.as_i64()) {
            if code != 0 && data.get("usage_summary").is_none() {
                return Err(AppError::new(
                    ErrorKind::Provider,
                    data.get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("用量接口返回失败"),
                ));
            }
        }
        let summary_data = data.get("usage_summary");
        let total = summary_data
            .and_then(|v| v.get("total_amount"))
            .and_then(|v| v.as_f64());
        let consumed = summary_data
            .and_then(|v| v.get("consumed_amount"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let balance = total.map(|t| (t - consumed).max(0.0));

        let mut packs: Vec<(f64, Option<chrono::DateTime<chrono::Local>>, String)> = data
            .get("user_entitlement_pack_list")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|pack| {
                let base = pack.get("entitlement_base_info")?;
                let limit = base
                    .pointer("/quota/credits_limit")
                    .and_then(|v| v.as_f64())
                    .filter(|v| *v > 0.0)?;
                let expires = base.get("end_time").and_then(dt_of);
                let label = pack
                    .get("display_desc")
                    .and_then(|v| v.as_str())
                    .unwrap_or("积分包")
                    .to_string();
                Some((limit, expires, label))
            })
            .collect();
        packs.sort_by_key(|a| a.1);

        // 消耗顺序是「剩余有效期最短的批次优先」：按到期日从早到晚扣掉已用量，
        // 剩下的才是每个批次现在可用的部分。
        let now = chrono::Local::now();
        let mut left = consumed;
        let mut lots = Vec::new();
        for (mut amount, expires, label) in packs {
            if left >= amount {
                left -= amount;
                continue;
            }
            amount -= left;
            left = 0.0;
            if expires.map(|e| e <= now).unwrap_or(false) {
                continue;
            }
            lots.push(CreditLot {
                amount,
                expires_at: expires,
                granted_at: None,
                lot_key: String::new(),
                label,
            });
        }

        let mut summary = match balance {
            Some(b) => format!("剩余 {} 积分", trim_f64(b)),
            None => "无可用积分".into(),
        };
        if let Some(total) = total {
            if consumed > 0.0 {
                summary.push_str(&format!(
                    "（已用 {}/{}）",
                    trim_f64(consumed),
                    trim_f64(total)
                ));
            }
        }
        Ok((summary, balance, Some(lots)))
    }
}

/// 有的版本把字段包在 `data` 里，有的不包；两边都认。
fn unwrap(resp: Value) -> Value {
    if resp.get("checked_in").is_none() {
        if let Some(data) = resp.get("data").filter(|v| v.get("checked_in").is_some()) {
            return data.clone();
        }
    }
    resp
}

fn dt_of(value: &Value) -> Option<chrono::DateTime<chrono::Local>> {
    crate::app::timeutil::parse_ts(value)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.with_timezone(&chrono::Local))
}

fn truncate(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
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
            let ciphertext =
                cbc::Encryptor::<aes_gcm::aes::Aes128>::new_from_slices(&seed[..16], &seed[16..32])
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
        assert_eq!(
            TraeProvider::at(dir.path().to_path_buf())
                .scan(&mut |_| Ok(()))
                .unwrap(),
            0
        );
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
            .scan(&mut |acc| {
                rows.push(acc);
                Ok(())
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].identity.iid, "trae:u1");
        assert_eq!(rows[0].identity.name, "某人");
    }
}
