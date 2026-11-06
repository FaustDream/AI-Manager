//! WorkBuddy 国内版 / 国际版：读 `.info` 登录票据，并按「账号档案」换登录态。
//!
//! 和 Qoder 最大的不同：WorkBuddy 的 token 只是票据的一部分。客户端登录态 =
//! `<家目录>/.workbuddy(-ai)/app/session`（Chromium 的 Cookies / Local Storage）
//! + `storage/` + `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\*.info`。
//!
//! 只写 `.info` 也能启动，但 Chromium 那边还是上一个账号的会话，客户端会判成
//! 无效登录并跳登录页——这是 v1 用「档案包」而不是「只写票据」的原因。
//! 所以切换 = **存当前账号的档案 → 换入目标账号的档案（票据按字节原样写回）**。
//!
//! `.info` 里的 token / 昵称 / refresh 都是 `$wbEncrypted` 字段：AES-256-GCM，
//! 密钥由客户端内置常量推导，AAD 是一段带长度前缀的「组帧」字节（见 `wb_aad`）。
//! 少写一个长度前缀就解不开，且报错只会是「解密失败」。

use super::{Provider, ScannedAccount, Sink, SwitchOutcome};
use crate::app::accounts::credential::{iso_from_ms, Credential};
use crate::app::accounts::{uid_of, AccountIdentity};
use crate::app::activity::{TravelConfig, TravelLocation, TravelRecord, TravelState, TravelStatus};
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::net::{Http, HttpRequest};
use crate::app::{fsx, paths, process};
use crate::types::{trim_f64, ActionResult, CreditLot, CreditSnapshot, StatusResult};
use aes_gcm::aead::consts::U16;
use aes_gcm::aead::KeyInit;
use async_trait::async_trait;
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 客户端内置的静态密钥（不在账号库里，也不随账号变化）
const WB_SECRET: &str = "Sik9U5aXhCdwTVEwsEySDOmDoB9r9ntFxHF1fst9LQI=";

fn wb_key_bytes() -> [u8; 32] {
    let digest = Sha256::digest(WB_SECRET.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// 标准 GCM 固定 12 字节 nonce，但 Python 的 `AESGCM` 允许任意长度。
/// 实测客户端自己写的票据是 **12 字节**，而 v1 的 `seal_field` 写的是 **16 字节**——
/// 两种都得能解，否则换个版本就整份登录态读不出来。
type WbCipher16 = aes_gcm::AesGcm<aes_gcm::aes::Aes256, U16>;

fn wb_decrypt(nonce: &[u8], ciphertext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, Payload};
    let payload = Payload {
        msg: ciphertext,
        aad,
    };
    let plain = match nonce.len() {
        12 => aes_gcm::Aes256Gcm::new_from_slice(&wb_key_bytes())
            .expect("32 字节密钥固定合法")
            .decrypt(aes_gcm::Nonce::from_slice(nonce), payload),
        16 => WbCipher16::new_from_slice(&wb_key_bytes())
            .expect("32 字节密钥固定合法")
            .decrypt(aes_gcm::Nonce::<U16>::from_slice(nonce), payload),
        n => {
            return Err(AppError::new(
                ErrorKind::Crypto,
                format!("nonce 长度不支持：{n}"),
            ))
        }
    };
    plain.map_err(|_| AppError::new(ErrorKind::Crypto, "解密登录票据失败（密钥或组帧不匹配）"))
}

fn wb_key_id() -> String {
    hex(&Sha256::digest(wb_key_bytes()))[..16].to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `$wbEncrypted` 字段的 AAD。字节顺序与客户端一致：
/// `"WB-AAD\0" | 0x01 | len+tag | len+"sym-v1" | suite | len+keyId | framing | 0 | 0`
fn wb_aad(framing_tag: &[u8], framing_num: u8, key_id: &str, suite: u32) -> Vec<u8> {
    let mut aad = Vec::new();
    aad.extend_from_slice(b"WB-AAD\x00");
    aad.push(1);
    aad.extend_from_slice(&(framing_tag.len() as u32).to_be_bytes());
    aad.extend_from_slice(framing_tag);
    aad.extend_from_slice(&6u32.to_be_bytes());
    aad.extend_from_slice(b"sym-v1");
    aad.extend_from_slice(&suite.to_be_bytes());
    aad.extend_from_slice(&(key_id.len() as u32).to_be_bytes());
    aad.extend_from_slice(key_id.as_bytes());
    aad.extend_from_slice(&[framing_num, 0, 0]);
    aad
}

/// 解开一个 `$wbEncrypted` 字段；不是加密字段就原样返回。
/// 昵称也可能是加密字段——实测有账号的昵称就是完整手机号，解不开就显示不出来。
pub fn open_field(value: &Value) -> Result<String> {
    let Some(obj) = value.as_object() else {
        return Ok(value.as_str().unwrap_or_default().to_string());
    };
    if obj.get("$wbEncrypted").and_then(|v| v.as_i64()) != Some(1) {
        return Ok(value.as_str().unwrap_or_default().to_string());
    }
    if let Some(scheme) = obj.get("scheme").and_then(|v| v.as_str()) {
        if scheme != "sym-v1" {
            return Err(AppError::new(
                ErrorKind::Crypto,
                format!("不支持的加密方案 {scheme}"),
            ));
        }
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let envelope_text = obj
        .get("envelope")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "加密字段缺 envelope"))?;
    let envelope: Value = serde_json::from_slice(&engine.decode(envelope_text).map_err(|e| {
        AppError::new(ErrorKind::Invalid, format!("envelope 不是合法 base64：{e}"))
    })?)?;

    let key_id = envelope.get("keyId").and_then(|v| v.as_str()).unwrap_or("");
    if key_id != wb_key_id() {
        return Err(AppError::new(
            ErrorKind::Crypto,
            "密钥不匹配，WorkBuddy 版本可能已更新",
        ));
    }
    let suite = envelope.get("suite").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let decode = |key: &str| -> Result<Vec<u8>> {
        let text = envelope.get(key).and_then(|v| v.as_str()).unwrap_or("");
        engine
            .decode(text)
            .map_err(|e| AppError::new(ErrorKind::Invalid, format!("{key} 不是合法 base64：{e}")))
    };

    let nonce = decode("nonce")?;
    let mut ciphertext = decode("ciphertext")?;
    ciphertext.extend_from_slice(&decode("authTag")?);

    let plain = wb_decrypt(&nonce, &ciphertext, &wb_aad(b"WBEV1", 2, key_id, suite))?;
    String::from_utf8(plain)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("票据不是 UTF-8：{e}")))
}

pub struct WorkBuddyProvider {
    pub intl: bool,
    /// `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth`
    auth_dir: Option<PathBuf>,
    /// 客户端数据目录，`~/.workbuddy` 或 `~/.workbuddy-ai`
    data_root: Option<PathBuf>,
    /// 账号档案根，默认 `<data>/profiles`
    pack_root: PathBuf,
    /// 需要检查是否在运行的进程名。测试可置空（否则本机开着客户端就跑不了切换用例）
    processes: &'static [&'static str],
}

impl WorkBuddyProvider {
    pub fn new(intl: bool) -> Self {
        Self {
            intl,
            auth_dir: default_auth_dir(),
            data_root: default_data_root(intl),
            pack_root: paths::data_dir().join("profiles"),
            processes: if intl {
                &["WorkBuddyAI.exe"]
            } else {
                &["WorkBuddy.exe", "CodeBuddy CN.exe", "CodeBuddy.exe"]
            },
        }
    }

    /// 指定全部路径（测试与迁移期用）
    pub fn at(intl: bool, auth_dir: PathBuf, data_root: PathBuf, pack_root: PathBuf) -> Self {
        let mut provider = Self::new(intl);
        provider.auth_dir = Some(auth_dir);
        provider.data_root = Some(data_root);
        provider.pack_root = pack_root;
        provider
    }

    /// 指定要检查的进程名（测试用空列表跳过「客户端是否在运行」这一步）
    pub fn with_processes(mut self, names: &'static [&'static str]) -> Self {
        self.processes = names;
        self
    }

    pub fn platform_key(&self) -> &'static str {
        "workbuddy"
    }

    pub fn edition(&self) -> &'static str {
        if self.intl {
            "intl"
        } else {
            "cn"
        }
    }

    pub fn file_name(&self) -> &'static str {
        if self.intl {
            "workbuddy-desktop-ai.info"
        } else {
            "workbuddy-desktop.info"
        }
    }

    pub fn process_names(&self) -> &'static [&'static str] {
        self.processes
    }

    pub fn live_ticket(&self) -> Option<PathBuf> {
        Some(self.auth_dir.as_ref()?.join(self.file_name()))
    }

    /// 客户端当前登录的 uid（**不**带 `@intl` 后缀）。
    pub fn current_uid(&self) -> Option<String> {
        let path = self.live_ticket()?;
        read_ticket_uid(&path)
    }

    /// 归属该版本的全部票据文件：主文件 + 客户端自己留的时间戳备份。
    /// 备份里带着当时的 `sessionState`，是「切回旧账号不弹登录页」的现成材料。
    fn ticket_files(&self) -> Vec<PathBuf> {
        let Some(dir) = self.auth_dir.as_ref().filter(|d| d.is_dir()) else {
            return Vec::new();
        };
        let base = self.file_name();
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // 同名主文件，或带时间戳/备份后缀的同族文件；`.json` 之类一律不碰
                name == base || (name.starts_with(base) && !name.ends_with(".json"))
            })
            .collect();
        // 主文件永远第一，备份按时间从新到旧——同一账号多份票据时取「当前登录的那份」
        let base = self.file_name();
        files.sort_by(|a, b| {
            let primary = |p: &PathBuf| p.file_name().and_then(|n| n.to_str()) != Some(base);
            primary(a).cmp(&primary(b)).then_with(|| {
                b.metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .cmp(&a.metadata().and_then(|m| m.modified()).ok())
            })
        });
        files
    }

    fn pack_dir(&self, uid: &str) -> PathBuf {
        self.pack_root.join(self.edition()).join(uid)
    }

    /// 档案是否可用（登录态 + 票据）。缺票据的档案切过去一定会停在登录页。
    pub fn pack_ready(&self, uid: &str) -> bool {
        self.pack_source(uid).is_some()
    }

    /// 可用的档案目录：正式目录优先；只剩 `.tmp` 时先把它认领回来。
    ///
    /// `.tmp` 是 `save_pack` 的中间产物。旧实现是「删掉正式目录 → rename tmp」，
    /// rename 一旦失败（客户端句柄未释放 / 杀软扫描），正式档案就消失了，
    /// 只留下一个内容完整的 tmp，而 `pack_ready` 只认正式目录——账号从此切不过去。
    /// 把 tmp 认领回来，是唯一既不丢数据、又能让这些账号恢复切换的办法。
    fn pack_source(&self, uid: &str) -> Option<PathBuf> {
        let dst = self.pack_dir(uid);
        if is_pack_complete(&dst) {
            return Some(dst);
        }
        let tmp = dst.with_extension("tmp");
        if !is_pack_complete(&tmp) {
            return None;
        }
        // 提升成正式目录；仍被占用就继续读 tmp，至少这一次切换能做
        if fsx::remove_path(&dst).is_ok() && std::fs::rename(&tmp, &dst).is_ok() {
            return Some(dst);
        }
        Some(tmp)
    }

    /// 把**当前**客户端登录态存成 uid 的档案（添加账号前调用，避免退出登录后档案丢失）。
    pub fn save_current_as_pack(&self, uid: &str) -> Result<PathBuf> {
        self.save_pack(uid)
    }

    /// 把**当前**客户端登录态存成 uid 的档案。
    fn save_pack(&self, uid: &str) -> Result<PathBuf> {
        let Some(root) = self.data_root.as_ref().filter(|p| p.exists()) else {
            return Err(AppError::new(
                ErrorKind::Io,
                "没有找到 WorkBuddy 的客户端数据目录",
            ));
        };
        let dst = self.pack_dir(uid);
        // 先写临时目录再整体换名：拷贝中途失败不会毁掉已有档案
        let tmp = dst.with_extension("tmp");
        fsx::remove_path(&tmp)?;
        std::fs::create_dir_all(&tmp)?;

        // 拷贝失败必须清掉半截 tmp：留着它下一次会被当成「完整档案」认领
        if let Err(e) = self.copy_pack_contents(root, &tmp, uid) {
            let _ = fsx::remove_path(&tmp);
            return Err(e);
        }

        // 三段式换名：dst → old，tmp → dst，成功再删 old。
        // 旧实现「先删 dst 再 rename」在任何一步失败时都会同时丢掉新旧两份档案。
        let old = dst.with_extension("old");
        let _ = fsx::remove_path(&old);
        let had_old = dst.exists();
        if had_old {
            if let Err(e) = std::fs::rename(&dst, &old) {
                let _ = fsx::remove_path(&tmp);
                return Err(AppError::new(
                    ErrorKind::Io,
                    format!("替换登录态档案失败（旧档案保留）：{e}"),
                ));
            }
        }
        if let Err(e) = std::fs::rename(&tmp, &dst) {
            let _ = fsx::remove_path(&tmp);
            if had_old {
                let _ = std::fs::rename(&old, &dst);
            }
            return Err(AppError::new(
                ErrorKind::Io,
                format!("替换登录态档案失败（已回滚到旧档案）：{e}"),
            ));
        }
        let _ = fsx::remove_path(&old);
        Ok(dst)
    }

    /// 拷贝会话 + storage + 票据到 `tmp`。抽出来是为了让调用方统一清掉半截目录。
    fn copy_pack_contents(&self, root: &Path, tmp: &Path, uid: &str) -> Result<()> {
        let session = root.join("app/session");
        if session.exists() {
            fsx::copy_profile_skipping_caches(&session, &tmp.join("app/session"))?;
        }
        // storage 下每个账号各有一份 user-<uid>-personal，仍是登录态相关，保持全量拷贝
        let storage = root.join("storage");
        if storage.exists() {
            fsx::copy_dir_merge(&storage, &tmp.join("storage"))?;
        }
        // 票据只在这个账号**自己**登录时才存：拿别人的票据存档，以后恢复就是「档案 A、票据 B」
        let ticket = self.live_ticket();
        let mut has_auth = false;
        if let Some(ticket) = ticket.as_ref().filter(|p| p.exists()) {
            if read_ticket_uid(ticket).as_deref() == Some(uid) {
                std::fs::create_dir_all(tmp)?;
                std::fs::copy(ticket, tmp.join("auth.info"))?;
                has_auth = true;
            }
        }
        let meta = json!({
            "uid": uid,
            "label": self.label(),
            "saved_at": chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "has_auth": has_auth,
        });
        std::fs::write(tmp.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
        Ok(())
    }

    /// 把某账号的档案恢复到客户端数据目录。
    fn restore_pack(&self, uid: &str) -> Result<()> {
        let Some(root) = self.data_root.as_ref() else {
            return Err(AppError::new(
                ErrorKind::Io,
                "没有找到 WorkBuddy 的客户端数据目录",
            ));
        };
        let src = self.pack_source(uid).ok_or_else(|| {
            AppError::new(
                ErrorKind::SessionStateMissing,
                "本机没有这个账号完整的登录态档案。请先在客户端里登录一次该账号，\
                 关闭客户端后会自动补存，之后切换就不需要重新登录。",
            )
        })?;
        // 登录态是全局的，必须整体替换；storage 里有每个账号自己的 user-<uid>-personal，
        // 只能合并——整体替换会把其他账号的本地数据一起删掉。
        // 档案不含 Chromium 缓存，整体替换会顺带清掉上一个账号留下的缓存，客户端自己重建；
        // 留着反而可能把上个账号的私有 HTTP 响应带给新账号。
        fsx::replace_dir(&src.join("app/session"), &root.join("app/session"))?;
        if src.join("storage").is_dir() {
            fsx::copy_dir_merge(&src.join("storage"), &root.join("storage"))?;
        }
        // 票据按字节原样写回：sessionState 是服务端签发的，重建一个随机值客户端不认
        let ticket = self.live_ticket();
        if let Some(ticket) = ticket {
            let bytes = std::fs::read(src.join("auth.info"))?;
            fsx::write_atomic(&ticket, &bytes)?;
        }
        Ok(())
    }
}

/// 档案可用 = 会话目录 + 票据文件都在。目录名对不对由调用方负责。
fn is_pack_complete(dir: &Path) -> bool {
    dir.join("app/session").is_dir() && dir.join("auth.info").is_file()
}

fn read_ticket_uid(path: &Path) -> Option<String> {
    let text = std::fs::read(path).ok()?;
    let data: Value = serde_json::from_slice(&text).ok()?;
    data.pointer("/account/uid")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[async_trait]
impl Provider for WorkBuddyProvider {
    fn platform(&self) -> &'static str {
        self.platform_key()
    }

    fn label(&self) -> &'static str {
        if self.intl {
            "WorkBuddy 国际版"
        } else {
            "WorkBuddy 国内版"
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

const API_CN: &str = "https://copilot.tencent.com/v2/billing/meter";
const API_INTL: &str = "https://www.workbuddy.ai/v2/billing/meter";
const USER_AGENT: &str = "Mozilla/5.0 CodeBuddy/5.6.0";

/// 猫猫旅行走 Web 域名：实测 `www.workbuddy.cn` 与 `copilot.tencent.com`
/// 都接受同一份 Bearer 令牌，这里固定用文档里的 Web 域名，减少一处漂移。
const TRAVEL_BASE: &str = "https://www.workbuddy.cn";
const TRAVEL_INFO: &str = "/activity/growth/buddy/info";
const TRAVEL_CONFIG: &str = "/activity/growth/buddy/travel/config";
const TRAVEL_STATUS: &str = "/activity/growth/buddy/travel/status";
const TRAVEL_RECORDS: &str = "/activity/growth/buddy/travel/records";
const TRAVEL_CLAIM: &str = "/activity/growth/buddy/travel/claim";
const TRAVEL_DEPART: &str = "/activity/growth/buddy/travel/depart";

/// 成长中心（个人中心 /profile/growth-center 背后的接口）。
/// 任务列表在 `/v2` 下，其余在 `/activity/growth` 下 —— 以 Web 端 bundle 里的
/// 实际路径为准，不要「统一加前缀」。
const CORE_BASE: &str = "https://copilot.tencent.com";

/// 平台奖励积分汇总（个人中心「套餐与积分」的明细）。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct RewardCredits {
    /// 服务端口径的总积分（`TotalDosage`）
    pub total: f64,
    /// 按包名聚合：还剩多少、一共发过多少、几批、最晚什么时候到期
    pub packages: Vec<RewardPackage>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub refreshed_at: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct RewardPackage {
    pub name: String,
    /// 本包剩余
    pub remain: f64,
    /// 本包总量（发放时的容量）
    pub size: f64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: i64,
    /// 最晚一批的到期时刻（毫秒）；国际版字段不可靠时为 null
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires_at: Option<i64>,
}

const GROWTH_TASKS: &str = "/v2/activity/growth/tasks";
const GROWTH_HEATMAP: &str = "/activity/growth/heatmap";
const GROWTH_STREAK: &str = "/activity/growth/streak";

impl WorkBuddyProvider {
    fn api_base(&self) -> &'static str {
        if self.intl {
            API_INTL
        } else {
            API_CN
        }
    }

    fn headers(&self, cred: &Credential) -> Vec<(String, String)> {
        vec![
            ("Authorization".into(), format!("Bearer {}", cred.token)),
            (
                "X-User-Id".into(),
                cred.meta_str("uid").unwrap_or("").to_string(),
            ),
            (
                "X-Domain".into(),
                cred.meta_str("domain").unwrap_or("").to_string(),
            ),
            ("Content-Type".into(), "application/json".into()),
            ("User-Agent".into(), cred.user_agent(USER_AGENT)),
        ]
    }

    // ------------------------------------------------------------ 成长中心

    /// 成长任务列表（只读）：每条任务的领取条件与完成状态都由服务端判定。
    pub async fn growth_tasks(&self, cred: &Credential, http: &dyn Http) -> Result<Value> {
        self.travel_call(cred, http, GROWTH_TASKS, None).await
    }

    /// 活跃地图（只读）：`month` 形如 `2026-11`，返回该月每天的活跃标记。
    pub async fn growth_heatmap(
        &self,
        cred: &Credential,
        http: &dyn Http,
        month: &str,
    ) -> Result<Value> {
        let path = format!("{GROWTH_HEATMAP}?month={month}");
        self.travel_call(cred, http, &path, None).await
    }

    /// 连续活跃天数（只读）。
    pub async fn growth_streak(&self, cred: &Credential, http: &dyn Http) -> Result<Value> {
        self.travel_call(cred, http, GROWTH_STREAK, None).await
    }

    /// 核心 API（`copilot.tencent.com`）调用：客户端 bundle 里 routeId=`core-api`，
    /// 与 `www.workbuddy.cn` 的活动域名不同，鉴权头是同一套。
    pub async fn core_call(
        &self,
        cred: &Credential,
        http: &dyn Http,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let text = self.core_text(cred, http, path, body).await?;
        serde_json::from_str(&text).map_err(|_| {
            AppError::new(
                ErrorKind::Provider,
                format!(
                    "{path} 返回的不是 JSON：{}",
                    text.chars().take(120).collect::<String>()
                ),
            )
        })
    }

    /// 同 `core_call`，但返回原始文本（对话接口只支持 SSE 流，不能当 JSON 解析）。
    pub async fn core_text(
        &self,
        cred: &Credential,
        http: &dyn Http,
        path: &str,
        body: Option<&Value>,
    ) -> Result<String> {
        let url = format!("{CORE_BASE}{path}");
        let mut req = match body {
            Some(_) => HttpRequest::post(url.as_str()),
            None => HttpRequest::get(url.as_str()),
        };
        req.headers = self.headers(cred);
        if let Some(body) = body {
            req.body = Some(body.to_string());
        }
        let resp = http.send(req).await?;
        if resp.status == 401 || resp.status == 403 {
            return Err(AppError::new(
                ErrorKind::AuthExpired,
                "HTTP 401/403（登录态可能过期）",
            ));
        }
        if resp.status >= 400 {
            // 带上服务端原话：这条路是逆向出来的，400/422 的具体原因比状态码有用得多
            let hint: String = resp.body.chars().take(200).collect();
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("HTTP {}（{path}）：{hint}", resp.status),
            ));
        }
        Ok(resp.body)
    }

    /// 旅行接口统一入口：GET/POST + 鉴权头 + 业务码归一化。
    /// 写操作（claim/depart）也只经过这里，避免再写一套状态码判断。
    async fn travel_call(
        &self,
        cred: &Credential,
        http: &dyn Http,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let url = format!("{TRAVEL_BASE}{path}");
        let mut req = match body {
            Some(_) => HttpRequest::post(url.as_str()),
            None => HttpRequest::get(url.as_str()),
        };
        req.headers = self.headers(cred);
        if let Some(body) = body {
            req.body = Some(body.to_string());
        }
        let resp = http.send(req).await?;
        match resp.status {
            401 | 403 => {
                return Err(AppError::new(
                    ErrorKind::AuthExpired,
                    "HTTP 401/403（登录态可能过期）",
                ))
            }
            429 => {
                return Err(AppError::new(
                    ErrorKind::RateLimited,
                    "平台限流（HTTP 429）",
                ))
            }
            status if status >= 500 => {
                return Err(AppError::new(ErrorKind::Provider, format!("HTTP {status}")))
            }
            status if status >= 400 => {
                return Err(AppError::new(ErrorKind::Provider, format!("HTTP {status}")))
            }
            _ => {}
        }
        let body = resp.json()?;
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
        if code != 0 {
            let message = body
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("请求失败");
            let kind = if message.contains("未解锁")
                || message.contains("未开放")
                || message.contains("已结束")
            {
                ErrorKind::ActivityClosed
            } else {
                ErrorKind::Provider
            };
            return Err(AppError::new(
                kind,
                crate::app::providers::hints::with_hint(format!("{message}（code={code}）"), code),
            ));
        }
        Ok(body)
    }

    async fn post(
        &self,
        cred: &Credential,
        http: &dyn Http,
        path: &str,
        body: &Value,
    ) -> Result<Value> {
        let url = format!("{}/{path}", self.api_base());
        let req = HttpRequest::post(url)
            .headers(&self.headers(cred))
            .json_body(body);
        let resp = http.send(req).await?;
        if resp.status == 401 {
            return Err(AppError::new(
                ErrorKind::AuthExpired,
                "HTTP 401（登录态可能过期）",
            ));
        }
        if resp.status >= 500 {
            return Err(AppError::new(
                ErrorKind::Provider,
                format!("HTTP {}", resp.status),
            ));
        }
        resp.json()
    }

    /// 新版签到看板。活动未开放时返回 404 / 4xx，这一层只表示「没读到」。
    async fn activity_status(&self, cred: &Credential, http: &dyn Http) -> Result<Option<Value>> {
        let url = format!("{}/checkin-activity-status", self.api_base());
        let req = HttpRequest::post(url)
            .headers(&self.headers(cred))
            .json_body(&json!({}));
        let resp = http.send(req).await?;
        if resp.status == 401 {
            return Err(AppError::new(
                ErrorKind::AuthExpired,
                "HTTP 401（登录态可能过期）",
            ));
        }
        if resp.status >= 400 {
            return Ok(None);
        }
        let Ok(body) = resp.json() else {
            return Ok(None);
        };
        if body.get("code").and_then(|v| v.as_i64()) == Some(0) {
            return Ok(body.get("data").filter(|v| v.is_object()).cloned());
        }
        Ok(None)
    }

    /// 平台奖励积分明细（个人中心「套餐与积分」那一页的数据）。
    ///
    /// 与 `resource_lots` 的分工：那个只给「还能用的批次」用于算余额，
    /// 这个按包名聚合，回答「这些积分是哪来的、还剩多少、什么时候到期」。
    pub async fn reward_credits(
        &self,
        cred: &Credential,
        http: &dyn Http,
    ) -> Result<RewardCredits> {
        let body = json!({
            "PageNumber": 1,
            "PageSize": 200,
            "ProductCode": "p_tcaca",
            "Status": [0, 3],
        });
        let payload = self.post(cred, http, "get-user-resource", &body).await?;
        let data = payload
            .pointer("/data/Response/Data")
            .or_else(|| payload.pointer("/data/Data"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let items = data
            .get("Accounts")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut grouped: std::collections::BTreeMap<String, RewardPackage> =
            std::collections::BTreeMap::new();
        for item in &items {
            let remain = [
                "CycleCapacityRemainPrecise",
                "CycleCapacityRemain",
                "CapacityRemainPrecise",
                "CapacityRemain",
            ]
            .iter()
            .find_map(|key| item.get(*key).and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
            // 用完了的包不占版面：列表里 66 个包，真正还有余额的只有一半
            if remain <= 0.0 {
                continue;
            }
            let size = [
                "CycleCapacitySizePrecise",
                "CycleCapacitySize",
                "CapacitySizePrecise",
                "CapacitySize",
            ]
            .iter()
            .find_map(|key| item.get(*key).and_then(|v| v.as_f64()))
            .unwrap_or(remain);
            let expires = item
                .get("DeductionEndTime")
                .and_then(dt_of)
                .or_else(|| item.get("CycleEndTime").and_then(local_text))
                .or_else(|| item.get("ExpiredTime").and_then(dt_of))
                .map(|dt| dt.timestamp_millis());
            let name = item
                .get("PackageName")
                .and_then(|v| v.as_str())
                .unwrap_or("积分包")
                .to_string();
            let entry = grouped.entry(name.clone()).or_insert(RewardPackage {
                name,
                remain: 0.0,
                size: 0.0,
                count: 0,
                expires_at: None,
            });
            entry.remain += remain;
            entry.size += size;
            entry.count += 1;
            // 同一包名可能有多批，取最近到期的那批（用户关心「最晚能用到什么时候」）
            entry.expires_at = match (entry.expires_at, expires) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (None, b) => b,
                (a, None) => a,
            };
        }

        let mut packages: Vec<RewardPackage> = grouped.into_values().collect();
        packages.sort_by(|a, b| {
            b.remain
                .partial_cmp(&a.remain)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(RewardCredits {
            total: data
                .get("TotalDosage")
                .and_then(|v| v.as_f64())
                .unwrap_or_else(|| packages.iter().map(|p| p.remain).sum()),
            packages,
            refreshed_at: chrono::Local::now().timestamp_millis(),
        })
    }

    /// 资源包批次。**不要带 `PackageEndTimeRange`**：部分账号的活跃资源包不在那个
    /// 区间里，服务端会只返回历史包，看起来就是「查不到积分」。
    async fn resource_lots(&self, cred: &Credential, http: &dyn Http) -> Result<Vec<CreditLot>> {
        let body = json!({
            "PageNumber": 1,
            "PageSize": 200,
            "ProductCode": "p_tcaca",
            "Status": [0, 3],
        });
        let payload = self.post(cred, http, "get-user-resource", &body).await?;
        let data = payload
            .pointer("/data/Response/Data")
            .or_else(|| payload.pointer("/data/Data"))
            .cloned()
            .unwrap_or_else(|| json!({}));

        let now = chrono::Local::now();
        let mut lots: Vec<CreditLot> = data
            .get("Accounts")
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|item| {
                // Cycle* 是本周期剩余，Capacity* 是整个套餐周期的总容量：只有「本期已
                // 消耗」时两者才不同（体验版常见 Capacity 500 / CycleRemain 0）。
                // 所以必须先取 Cycle，字段缺失才回退 Capacity，否则会把用完的当剩余。
                let amount = [
                    "CycleCapacityRemainPrecise",
                    "CapacityRemainPrecise",
                    "CycleCapacityRemain",
                    "CapacityRemain",
                ]
                .iter()
                .find_map(|key| item.get(*key).and_then(|v| v.as_f64()))
                .filter(|v| *v > 0.0)?;
                let expires = if self.intl {
                    // 国际版的资源包字段里没有可靠的「到期」：CycleEndTime 是周期重置时间，
                    // 拿它当到期会显示一个错的日期。宁可空着，也不猜。
                    None
                } else {
                    item.get("DeductionEndTime")
                        .and_then(dt_of)
                        .or_else(|| item.get("CycleEndTime").and_then(local_text))
                        .or_else(|| item.get("ExpiredTime").and_then(dt_of))
                };
                if expires.map(|e| e <= now).unwrap_or(false) {
                    return None;
                }
                Some(CreditLot {
                    amount,
                    expires_at: expires,
                    granted_at: None,
                    lot_key: String::new(),
                    label: item
                        .get("PackageName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("积分包")
                        .to_string(),
                })
            })
            .collect();
        lots.sort_by_key(|a| a.expires_at);
        Ok(lots)
    }
}

fn dt_of(value: &Value) -> Option<chrono::DateTime<chrono::Local>> {
    crate::app::timeutil::parse_ts(value)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.with_timezone(&chrono::Local))
}

/// 资源包里的 `CycleEndTime` 是本地时间字符串 `2026-11-01 10:00:00`（无时区）。
fn local_text(value: &Value) -> Option<chrono::DateTime<chrono::Local>> {
    let text = value.as_str()?;
    let naive = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").ok()?;
    naive.and_local_timezone(chrono::Local).single()
}

/// 切换登录：存档当前账号 → 换入目标档案。
pub fn switch_to(
    provider: &WorkBuddyProvider,
    account_id: &str,
    name: &str,
) -> Result<SwitchOutcome> {
    let uid = uid_of(account_id);
    // 先判档案：缺档案时连客户端都不用关，直接告诉用户去登录一次
    if !provider.pack_ready(uid) {
        // 先判档案：没有档案就不要动当前登录态，一个字节都不写
        return Err(AppError::new(
            ErrorKind::SessionStateMissing,
            format!(
                "本机没有 {name} 完整的登录态档案。请先在客户端里登录一次该账号，\
                 关闭客户端后会自动补存票据，之后再切换就不用重新登录。"
            ),
        ));
    }

    if provider.current_uid().as_deref() == Some(uid) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "这个账号就是客户端当前登录的账号，不需要切换",
        ));
    }
    if process::process_running(provider.process_names()) {
        return Err(AppError::new(
            ErrorKind::ClientRunning,
            format!("请先关闭 {} 客户端再切换", provider.label()),
        ));
    }

    // 1) 存档当前账号（不是目标账号才存；否则会用目标票据覆盖自己）
    let current = provider.current_uid();
    let mut notes = Vec::new();
    if let Some(current) = current.as_ref().filter(|c| *c != uid) {
        match provider.save_pack(current) {
            Ok(_) => notes.push("已存档当前账号的登录态，随时可以切回来".to_string()),
            // 存档失败不阻断切换：目标档案是完整的，用户的诉求是「切到目标账号」。
            // 但它是**部分失败**，必须按 Warn 落日志——以前跟着切换的成功一起记成
            // info，事后翻日志只会看到「切换成功」，看不出旧账号已经存不回来了。
            Err(e) => {
                crate::app::log::log(
                    crate::app::log::Level::Warn,
                    "providers",
                    format!(
                        "切换前存档当前账号 {current} 失败（切换继续）：{}",
                        e.message
                    ),
                );
                notes.push(format!("存档当前账号失败（切换继续）：{}", e.message));
            }
        }
    }

    // 2) 换入目标档案
    provider.restore_pack(uid)?;

    // 3) 验读：客户端认的是 .info 里的 uid，写回来对不上就等于没切
    if provider.current_uid().as_deref() != Some(uid) {
        return Err(AppError::new(
            ErrorKind::WriteFailed,
            "写入后校验失败：票据里的账号不是目标账号",
        ));
    }

    let mut message = format!("{} 已切换到 {name}", provider.label());
    if !notes.is_empty() {
        message.push_str(&format!("（{}）", notes.join("；")));
    }
    Ok(SwitchOutcome { message })
}

fn default_auth_dir() -> Option<PathBuf> {
    let local = std::env::var("LOCALAPPDATA").ok()?;
    Some(
        PathBuf::from(local)
            .join("CodeBuddyExtension")
            .join("Data")
            .join("Public")
            .join("auth"),
    )
}

fn default_data_root(intl: bool) -> Option<PathBuf> {
    let home = paths::home();
    Some(home.join(if intl { ".workbuddy-ai" } else { ".workbuddy" }))
}

fn travel_location(value: &Value) -> Option<TravelLocation> {
    let obj = value.as_object()?;
    let id = obj.get("id").and_then(|v| v.as_i64())?;
    Some(TravelLocation {
        id,
        code: obj
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        description: obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        duration_hours: obj.get("duration_hours").and_then(|v| v.as_i64()),
        duration_hours_min: obj.get("duration_hours_min").and_then(|v| v.as_i64()),
        duration_hours_max: obj.get("duration_hours_max").and_then(|v| v.as_i64()),
        reward_credit_min: obj.get("reward_credit_min").and_then(|v| v.as_f64()),
        reward_credit_max: obj.get("reward_credit_max").and_then(|v| v.as_f64()),
    })
}

fn normalize_travel_status(data: &Value) -> TravelStatus {
    let daily_limit_reached = data
        .get("daily_limit_reached")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let raw = data
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let state = match (raw, daily_limit_reached) {
        ("idle", true) | ("limit", _) => TravelState::Limit,
        ("idle", false) => TravelState::Idle,
        ("traveling", _) => TravelState::Traveling,
        ("arrived", _) => TravelState::Arrived,
        _ => TravelState::Unknown,
    };

    TravelStatus {
        state: Some(state),
        location: data.get("location").and_then(travel_location),
        arrive_at: data.get("arrive_at").and_then(positive_ts),
        depart_at: data.get("depart_at").and_then(positive_ts),
        server_now: data.get("server_now").and_then(positive_ts),
        reward_credit: data.get("reward_credit").and_then(|v| v.as_f64()),
        duration_hours: data.get("duration_hours").and_then(|v| v.as_i64()),
        daily_limit_reached,
        letter: data
            .pointer("/letter/text")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        buddy_name: None,
        record_id: data.get("record_id").and_then(|v| v.as_i64()),
    }
}

fn normalize_travel_record(value: &Value) -> TravelRecord {
    TravelRecord {
        record_id: value.get("record_id").and_then(|v| v.as_i64()),
        travel_date: value
            .get("travel_date")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        location_name: value
            .pointer("/location/name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        reward_credit: value.get("reward_credit").and_then(|v| v.as_f64()),
        depart_at: value.get("depart_at").and_then(positive_ts),
        arrive_at: value.get("arrive_at").and_then(positive_ts),
        claimed_at: value.get("claimed_at").and_then(positive_ts),
        letter: value
            .pointer("/letter/text")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

/// 0 在这套接口里表示「没有时间」（idle 状态的 arrive_at 就是 0），
/// 不能当成 1970-01-01，否则界面会显示一个看起来很真的假日期。
fn positive_ts(value: &Value) -> Option<i64> {
    crate::app::timeutil::parse_ts(value).filter(|ms| *ms > 0)
}

fn local_hhmm(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::net::MockHttp;
    use aes_gcm::aead::Aead;

    fn provider(dir: &Path) -> WorkBuddyProvider {
        WorkBuddyProvider::at(
            false,
            dir.join("auth"),
            dir.join("client"),
            dir.join("profiles"),
        )
    }

    /// 用客户端的固定密钥造一个 `$wbEncrypted` 字段（等价于 v1 的 `seal_field`）
    fn seal_with(nonce: &[u8], value: &str) -> Value {
        let key_id = wb_key_id();
        let aad = wb_aad(b"WBEV1", 2, &key_id, 1);
        let sealed = match nonce.len() {
            12 => aes_gcm::Aes256Gcm::new_from_slice(&wb_key_bytes())
                .unwrap()
                .encrypt(
                    aes_gcm::Nonce::from_slice(nonce),
                    aes_gcm::aead::Payload {
                        msg: value.as_bytes(),
                        aad: &aad,
                    },
                )
                .unwrap(),
            16 => WbCipher16::new_from_slice(&wb_key_bytes())
                .unwrap()
                .encrypt(
                    aes_gcm::Nonce::<U16>::from_slice(nonce),
                    aes_gcm::aead::Payload {
                        msg: value.as_bytes(),
                        aad: &aad,
                    },
                )
                .unwrap(),
            _ => unreachable!(),
        };
        let (ciphertext, tag) = sealed.split_at(sealed.len() - 16);
        let envelope = json!({
            "suite": 1,
            "keyId": key_id,
            "nonce": base64::engine::general_purpose::STANDARD.encode(nonce),
            "authTag": base64::engine::general_purpose::STANDARD.encode(tag),
            "ciphertext": base64::engine::general_purpose::STANDARD.encode(ciphertext),
        });
        json!({
            "$wbEncrypted": 1,
            "envelope": base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&envelope).unwrap()),
        })
    }

    /// 客户端实际写的票据用 12 字节 nonce
    fn seal(value: &str) -> Value {
        seal_with(&[5u8; 12], value)
    }

    fn ticket(uid: &str, token: &str, name: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "auth": {
                "accessToken": seal(token),
                "refreshToken": seal("rt"),
                "expiresAt": 1_700_000_000_000i64,
                "domain": "www.workbuddy.cn",
            },
            "account": { "uid": uid, "nickname": seal(name) },
        }))
        .unwrap()
    }

    #[test]
    fn encrypted_field_roundtrip() {
        let field = seal("17679130626");
        assert_eq!(open_field(&field).unwrap(), "17679130626");
    }

    #[test]
    fn both_nonce_lengths_are_supported() {
        // 客户端 12 字节；v1 的 seal_field 写 16 字节。两种都出现过
        assert_eq!(open_field(&seal_with(&[1u8; 12], "a")).unwrap(), "a");
        assert_eq!(open_field(&seal_with(&[1u8; 16], "b")).unwrap(), "b");
    }

    #[test]
    fn plain_field_passes_through() {
        assert_eq!(open_field(&json!("plain")).unwrap(), "plain");
        assert_eq!(open_field(&Value::Null).unwrap(), "");
    }

    #[test]
    fn headers_prefer_account_ua_and_fall_back_to_constant() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());

        let mut cred = Credential {
            token: "tk".into(),
            ..Default::default()
        };
        cred.put("ua", json!("Mozilla/5.0 Account/9.0"));
        let headers = p.headers(&cred);
        let ua = headers
            .iter()
            .find(|(key, _)| key == "User-Agent")
            .map(|(_, value)| value.as_str());
        assert_eq!(ua, Some("Mozilla/5.0 Account/9.0"));

        let old = Credential {
            token: "tk".into(),
            ..Default::default()
        };
        let headers = p.headers(&old);
        let ua = headers
            .iter()
            .find(|(key, _)| key == "User-Agent")
            .map(|(_, value)| value.as_str());
        assert_eq!(ua, Some(USER_AGENT), "旧凭据没有 ua 时回退常量");
    }

    #[test]
    fn tampered_field_is_an_error_not_a_panic() {
        let mut field = seal("x");
        let obj = field.as_object_mut().unwrap();
        obj.insert("envelope".into(), json!("bm90LWEtcmVhbC1lbnZlbG9wZQ=="));
        assert!(open_field(&field).is_err());
    }

    #[test]
    fn scan_reads_ticket_and_keeps_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        let raw = ticket("u1", "tk1", "凌致");
        std::fs::write(dir.path().join("auth/workbuddy-desktop.info"), &raw).unwrap();

        let mut found = Vec::new();
        let count = provider(dir.path())
            .scan(&mut |acc| {
                found.push(acc);
                Ok(())
            })
            .unwrap();

        assert_eq!(count, 1);
        assert_eq!(found[0].identity.name, "凌致");
        assert_eq!(found[0].identity.iid, "workbuddy:u1");
        assert!(found[0].identity.requires_checkin);

        let cred = Credential::parse(&found[0].secret).unwrap();
        assert_eq!(cred.token, "tk1");
        assert_eq!(cred.meta_str("uid"), Some("u1"));
        let stored = base64::engine::general_purpose::STANDARD
            .decode(cred.meta_str("ticket").unwrap())
            .unwrap();
        assert_eq!(stored, raw, "契约 8a：票据必须按字节保存");
    }

    #[test]
    fn duplicate_backups_collapse_to_one_account() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "new", "新"),
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join("auth/workbuddy-desktop.info.bak-20260101-000000"),
            ticket("u1", "old", "旧"),
        )
        .unwrap();

        let mut names = Vec::new();
        let count = provider(dir.path())
            .scan(&mut |acc| {
                names.push(acc.identity.name.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(count, 1, "同一账号的多份备份只能算一个");
        assert_eq!(names[0], "新", "应取最新的一份");
    }

    #[test]
    fn intl_ticket_is_not_required_to_checkin() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop-ai.info"),
            ticket("u2", "tk", "bob"),
        )
        .unwrap();
        let intl = WorkBuddyProvider::at(
            true,
            dir.path().join("auth"),
            dir.path().join("client"),
            dir.path().join("profiles"),
        );
        let mut rows = Vec::new();
        intl.scan(&mut |acc| {
            rows.push(acc);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows[0].identity.iid, "workbuddy:u2@intl");
        assert!(!rows[0].identity.requires_checkin, "国际版免签到");
    }

    #[test]
    fn missing_auth_dir_returns_zero() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(provider(dir.path()).scan(&mut |_| Ok(())).unwrap(), 0);
    }

    /// Chromium 的纯缓存不参与登录判定，却会被整目录拷进每个账号档案，
    /// 再被退出登录备份、手工快照各复制一遍。先固定「档案里只留登录相关文件」。
    #[test]
    fn save_pack_does_not_archive_chromium_caches() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let client = dir.path().join("client");
        let session = client.join("app/session");
        std::fs::create_dir_all(session.join("Cache")).unwrap();
        std::fs::create_dir_all(session.join("Code Cache")).unwrap();
        std::fs::create_dir_all(session.join("Local Storage")).unwrap();
        std::fs::write(session.join("Cache/data_1"), vec![0u8; 4096]).unwrap();
        std::fs::write(session.join("Code Cache/js"), b"compiled").unwrap();
        std::fs::write(session.join("Cookies"), b"session-cookie").unwrap();
        std::fs::write(session.join("Local Storage/leveldb"), b"tokens").unwrap();
        std::fs::create_dir_all(client.join("storage/skeleton")).unwrap();
        std::fs::write(
            client.join("storage/skeleton/account-snapshot.json"),
            b"{\"primary\":{}}",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "tk", "n"),
        )
        .unwrap();

        p.save_pack("u1").unwrap();
        let pack = p.pack_dir("u1");
        assert!(
            !pack.join("app/session/Cache").exists(),
            "纯缓存不该进账号档案"
        );
        assert!(
            !pack.join("app/session/Code Cache").exists(),
            "纯缓存不该进账号档案"
        );
        assert_eq!(
            std::fs::read(pack.join("app/session/Cookies")).unwrap(),
            b"session-cookie"
        );
        assert_eq!(
            std::fs::read(pack.join("app/session/Local Storage/leveldb")).unwrap(),
            b"tokens"
        );
        assert!(p.pack_ready("u1"));
    }

    /// 精简档案（不含缓存）仍要能完整换回登录态，票据按字节写回。
    #[test]
    fn restore_from_slim_pack_swaps_login_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let client = dir.path().join("client");
        let auth = dir.path().join("auth");
        std::fs::create_dir_all(&auth).unwrap();

        let pack = p.pack_dir("u1");
        std::fs::create_dir_all(pack.join("app/session/Local Storage")).unwrap();
        std::fs::create_dir_all(pack.join("storage/skeleton")).unwrap();
        std::fs::write(pack.join("app/session/Cookies"), b"B").unwrap();
        std::fs::write(pack.join("app/session/Local Storage/leveldb"), b"B-tokens").unwrap();
        std::fs::write(pack.join("storage/skeleton/account-snapshot.json"), b"{}").unwrap();
        let saved = ticket("u1", "tk", "n");
        std::fs::write(pack.join("auth.info"), &saved).unwrap();

        // 客户端当前是别的账号，还带着自己的缓存
        std::fs::create_dir_all(client.join("app/session/Cache")).unwrap();
        std::fs::write(client.join("app/session/Cache/data"), b"old").unwrap();
        std::fs::write(client.join("app/session/Cookies"), b"A").unwrap();
        std::fs::create_dir_all(client.join("storage/skeleton")).unwrap();
        std::fs::write(
            auth.join("workbuddy-desktop.info"),
            b"{\"account\":{\"uid\":\"u2\"}}",
        )
        .unwrap();

        p.restore_pack("u1").unwrap();
        assert_eq!(
            std::fs::read(auth.join("workbuddy-desktop.info")).unwrap(),
            saved,
            "契约 8a：票据按字节写回"
        );
        assert_eq!(
            std::fs::read(client.join("app/session/Cookies")).unwrap(),
            b"B"
        );
        assert_eq!(
            std::fs::read(client.join("app/session/Local Storage/leveldb")).unwrap(),
            b"B-tokens"
        );
        assert!(
            !client.join("app/session/Cache").exists(),
            "旧账号缓存不能串到新账号"
        );
    }

    /// 旧版 `save_pack` 在 rename 失败时会同时丢掉正式档案、只留下完整 tmp。
    /// 这些账号必须能被认领回来，否则永远报「本机没有完整登录态档案」。
    #[test]
    fn orphan_tmp_pack_is_claimed_back() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let tmp = p.pack_dir("u1").with_extension("tmp");
        std::fs::create_dir_all(tmp.join("app/session")).unwrap();
        std::fs::write(tmp.join("auth.info"), ticket("u1", "tk", "n")).unwrap();

        assert!(
            p.pack_ready("u1"),
            "内容完整的 tmp 必须被认领，否则这个账号永远切不过去"
        );
        assert!(
            p.pack_dir("u1").join("app/session").is_dir(),
            "认领后应提升为正式目录"
        );
        assert!(!tmp.exists(), "提升成功后 tmp 不该留着");
    }

    #[test]
    fn half_written_tmp_pack_is_not_claimed() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let tmp = p.pack_dir("u1").with_extension("tmp");
        std::fs::create_dir_all(tmp.join("app/session")).unwrap();
        // 缺 auth.info：切过去只会停在登录页，不能当成可用档案
        assert!(!p.pack_ready("u1"));
    }

    #[test]
    fn repeated_save_leaves_no_staging_dirs() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let client = dir.path().join("client");
        std::fs::create_dir_all(client.join("app/session")).unwrap();
        std::fs::write(client.join("app/session/Cookies"), b"c").unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "tk", "n"),
        )
        .unwrap();

        p.save_pack("u1").unwrap();
        p.save_pack("u1").unwrap();

        assert!(p.pack_ready("u1"));
        assert!(!p.pack_dir("u1").with_extension("tmp").exists());
        assert!(
            !p.pack_dir("u1").with_extension("old").exists(),
            "换名用的旧目录不能残留"
        );
    }

    #[test]
    fn save_and_restore_pack_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let client = dir.path().join("client");
        std::fs::create_dir_all(client.join("app/session/Local Storage")).unwrap();
        std::fs::create_dir_all(client.join("storage/skeleton")).unwrap();
        std::fs::write(client.join("app/session/Local Storage/leveldb"), b"cookies").unwrap();
        std::fs::write(
            client.join("storage/skeleton/account-snapshot.json"),
            b"{\"primary\":{}}",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "tk", "n"),
        )
        .unwrap();

        p.save_pack("u1").unwrap();
        assert!(p.pack_ready("u1"));
        let saved_ticket = std::fs::read(p.pack_dir("u1").join("auth.info")).unwrap();

        // 客户端换成别人：登录态被清空、票据被改写
        fsx::remove_path(&client.join("app/session")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            b"{\"account\":{\"uid\":\"u2\"}}",
        )
        .unwrap();

        p.restore_pack("u1").unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("auth/workbuddy-desktop.info")).unwrap(),
            saved_ticket,
            "契约 8a：票据必须按字节写回"
        );
        assert_eq!(
            std::fs::read(client.join("app/session/Local Storage/leveldb")).unwrap(),
            b"cookies"
        );
    }

    /// 完整切换：当前登录 A，目标 B 已有档案 → 换过去、A 被自动存档。
    #[test]
    fn switch_swaps_packs_and_archives_current_account() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path()).with_processes(&[]); // 本机可能正开着客户端
        let client = dir.path().join("client");
        let auth = dir.path().join("auth");
        std::fs::create_dir_all(client.join("app/session")).unwrap();
        std::fs::create_dir_all(client.join("storage/skeleton")).unwrap();
        std::fs::create_dir_all(&auth).unwrap();

        // 当前登录 A
        std::fs::write(client.join("app/session/cookies"), b"A-cookies").unwrap();
        std::fs::write(
            client.join("storage/skeleton/account-snapshot.json"),
            b"{\"primary\":{\"uid\":\"u1\"}}",
        )
        .unwrap();
        std::fs::write(
            auth.join("workbuddy-desktop.info"),
            ticket("u1", "tk-a", "A"),
        )
        .unwrap();

        // B 的档案（登录态 + 票据）
        let pack_b = p.pack_dir("u2");
        std::fs::create_dir_all(pack_b.join("app/session")).unwrap();
        std::fs::create_dir_all(pack_b.join("storage/skeleton")).unwrap();
        std::fs::write(pack_b.join("app/session/cookies"), b"B-cookies").unwrap();
        std::fs::write(
            pack_b.join("storage/skeleton/account-snapshot.json"),
            b"{\"primary\":{\"uid\":\"u2\"}}",
        )
        .unwrap();
        let ticket_b = ticket("u2", "tk-b", "B");
        std::fs::write(pack_b.join("auth.info"), &ticket_b).unwrap();

        let out = switch_to(&p, "u2", "B").unwrap();

        assert_eq!(
            p.current_uid().as_deref(),
            Some("u2"),
            "切换后客户端应登录 B"
        );
        assert_eq!(
            std::fs::read(auth.join("workbuddy-desktop.info")).unwrap(),
            ticket_b,
            "契约 8a：票据按字节写回"
        );
        assert_eq!(
            std::fs::read(client.join("app/session/cookies")).unwrap(),
            b"B-cookies"
        );
        assert!(p.pack_ready("u1"), "当前账号必须先被存档，否则切不回去");
        assert!(out.message.contains("存档"));
    }

    #[test]
    fn travel_status_normalizes_the_real_arrived_shape() {
        let data = json!({
            "state": "arrived",
            "buddy_id": 12314099,
            "record_id": 38593793,
            "location": {"id": 1, "code": "coffee", "name": "咖啡馆", "duration_hours": 3},
            "depart_at": 1793519788,
            "arrive_at": 1793530588,
            "server_now": 1793870943,
            "letter": {"id": 2, "text": "亲爱的铲屎官：……", "guide_text": "看看工具"},
            "daily_limit_reached": false,
            "duration_hours": 3,
            "reward_credit": 8
        });
        let status = normalize_travel_status(&data);
        assert_eq!(status.state, Some(TravelState::Arrived));
        assert_eq!(
            status.location.as_ref().map(|l| l.name.as_str()),
            Some("咖啡馆")
        );
        assert_eq!(
            status.arrive_at,
            Some(1_793_530_588_000),
            "秒级时间要转毫秒"
        );
        assert_eq!(status.reward_credit, Some(8.0));
        assert_eq!(status.record_id, Some(38_593_793));
        assert!(!status.daily_limit_reached);
        assert!(status.letter.unwrap().contains("铲屎官"));
    }

    #[test]
    fn travel_limit_wins_over_idle() {
        let data = json!({"state": "idle", "daily_limit_reached": true});
        assert_eq!(
            normalize_travel_status(&data).state,
            Some(TravelState::Limit),
            "额度用完不能当成可派出"
        );
        let data = json!({"state": "traveling", "arrive_at": 0});
        let status = normalize_travel_status(&data);
        assert_eq!(status.state, Some(TravelState::Traveling));
        assert_eq!(status.arrive_at, None, "0 表示没有时间，不能显示成 1970 年");
    }

    #[test]
    fn travel_location_and_record_parse_the_real_fields() {
        let location = travel_location(&json!({
            "id": 1, "code": "coffee", "name": "咖啡馆",
            "duration_hours_min": 1, "duration_hours_max": 4,
            "reward_credit_min": 5, "reward_credit_max": 10
        }))
        .unwrap();
        assert_eq!(location.name, "咖啡馆");
        assert_eq!(location.duration_hours_max, Some(4));
        assert_eq!(location.reward_credit_min, Some(5.0));

        let record = normalize_travel_record(&json!({
            "record_id": 34232265,
            "location": {"id": 1, "code": "coffee", "name": "咖啡馆"},
            "depart_at": 1793234984,
            "arrive_at": 1793242184,
            "claimed_at": 1793519786,
            "reward_credit": 7,
            "travel_date": "2026-10-29",
            "letter": {"text": "记录信件"}
        }));
        assert_eq!(record.location_name.as_deref(), Some("咖啡馆"));
        assert_eq!(record.reward_credit, Some(7.0));
        assert_eq!(record.travel_date.as_deref(), Some("2026-10-29"));
        assert!(record.claimed_at.is_some());
    }

    #[tokio::test]
    async fn travel_claim_and_depart_hit_the_expected_paths() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let mut cred = Credential {
            token: "tk".into(),
            ..Default::default()
        };
        cred.put("uid", json!("u1"));

        let http = MockHttp::new();
        let claim_url = format!("{TRAVEL_BASE}{TRAVEL_CLAIM}");
        http.on(
            &claim_url,
            200,
            r#"{"code":0,"msg":"OK","data":{"reward_credit":8}}"#,
        );
        let claimed = p.travel_claim(&cred, &http).await.unwrap();
        assert!(claimed.ok);
        assert_eq!(claimed.granted, Some(8.0));
        assert_eq!(http.call_count(&claim_url), 1);

        let depart_url = format!("{TRAVEL_BASE}{TRAVEL_DEPART}");
        http.on(
            &depart_url,
            200,
            r#"{"code":0,"msg":"OK","data":{"location":{"id":1,"name":"咖啡馆"},"arrive_at":1793530588}}"#,
        );
        let departed = p.travel_depart(&cred, &http, 1).await.unwrap();
        assert!(departed.ok);
        assert!(departed.message.contains("咖啡馆"));
        assert_eq!(http.call_count(&depart_url), 1);
    }

    #[tokio::test]
    async fn travel_reward_does_not_complete_daily_checkin() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        let mut cred = Credential {
            token: "tk".into(),
            ..Default::default()
        };
        cred.put("uid", json!("u1"));
        let http = MockHttp::new();
        http.on(
            &format!("{TRAVEL_BASE}{TRAVEL_CLAIM}"),
            200,
            r#"{"code":0,"data":{"reward_credit":8}}"#,
        );
        http.on(
            &format!("{}/checkin-activity-status", p.api_base()),
            200,
            r#"{"code":0,"data":{"active":true,"today_checked_in":false}}"#,
        );
        http.on(&format!("{}/get-user-resource", p.api_base()), 200,
            r#"{"code":0,"data":{"Data":{"Accounts":[{"CycleCapacityRemain":108,"PackageName":"旅行奖励"}]}}}"#);
        let daily_url = format!("{}/daily-checkin", p.api_base());
        http.on(&daily_url, 200, r#"{"code":0,"data":{"credit":10}}"#);
        assert_eq!(
            p.travel_claim(&cred, &http).await.unwrap().granted,
            Some(8.0)
        );
        let status = p.status(&cred, &http).await.unwrap();
        assert_eq!(status.checked_in, Some(false));
        assert_eq!(status.balance, Some(108.0));
        assert_eq!(http.call_count(&daily_url), 0);
        let result = p.checkin(&cred, &http).await.unwrap();
        assert!(result.ok && !result.already);
        assert_eq!(result.granted, Some(10.0));
        assert_eq!(http.call_count(&daily_url), 1);
    }

    #[test]
    fn intl_has_no_travel_activity() {
        let dir = tempfile::TempDir::new().unwrap();
        let intl = WorkBuddyProvider::at(
            true,
            dir.path().join("auth"),
            dir.path().join("client"),
            dir.path().join("profiles"),
        );
        assert!(!intl.travel_supported(), "国际版没有猫猫旅行");
        assert!(provider(dir.path()).travel_supported(), "国内版应支持");
    }

    #[test]
    fn switch_without_pack_touches_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = provider(dir.path());
        std::fs::create_dir_all(dir.path().join("auth")).unwrap();
        std::fs::write(
            dir.path().join("auth/workbuddy-desktop.info"),
            ticket("u1", "tk", "n"),
        )
        .unwrap();
        let before = std::fs::read(dir.path().join("auth/workbuddy-desktop.info")).unwrap();

        let err = switch_to(&p, "u9", "目标").unwrap_err();
        assert!(matches!(
            err.kind,
            crate::app::error::ErrorKind::SessionStateMissing
        ));
        assert_eq!(
            std::fs::read(dir.path().join("auth/workbuddy-desktop.info")).unwrap(),
            before,
            "没有档案时一个字节都不能写"
        );
    }
}
