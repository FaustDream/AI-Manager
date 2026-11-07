//! `.info` 字段解密：AES-256-GCM，密钥由客户端内置常量推导，
//! AAD 是一段带长度前缀的「组帧」字节（见 `wb_aad`）。少写一个长度前缀就解不开，
//! 且报错只会是「解密失败」——所以组帧逻辑一个字节都不能动。

use crate::app::error::{AppError, ErrorKind, Result};
use aes_gcm::aead::consts::U16;
use aes_gcm::aead::KeyInit;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

const WB_SECRET: &str = "Sik9U5aXhCdwTVEwsEySDOmDoB9r9ntFxHF1fst9LQI=";

pub(crate) fn wb_key_bytes() -> [u8; 32] {
    let digest = Sha256::digest(WB_SECRET.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// 标准 GCM 固定 12 字节 nonce，但 Python 的 `AESGCM` 允许任意长度。
/// 实测客户端自己写的票据是 **12 字节**，而 v1 的 `seal_field` 写的是 **16 字节**——
/// 两种都得能解，否则换个版本就整份登录态读不出来。
pub(crate) type WbCipher16 = aes_gcm::AesGcm<aes_gcm::aes::Aes256, U16>;

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

pub(crate) fn wb_key_id() -> String {
    hex(&Sha256::digest(wb_key_bytes()))[..16].to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `$wbEncrypted` 字段的 AAD。字节顺序与客户端一致：
/// `"WB-AAD\0" | 0x01 | len+tag | len+"sym-v1" | suite | len+keyId | framing | 0 | 0`
pub(crate) fn wb_aad(framing_tag: &[u8], framing_num: u8, key_id: &str, suite: u32) -> Vec<u8> {
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
