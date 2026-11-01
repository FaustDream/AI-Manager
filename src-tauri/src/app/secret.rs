//! DPAPI 封装：账号凭据按账号单独加密（v1 是整个 JSON 一起加密）。
//!
//! 为什么改成按账号：读单个账号不必解开全部，且内存里同时存在的明文从
//! 「所有账号」降到「一个账号」。DPAPI 与当前 Windows 用户绑定，换用户/换机解不开。

use crate::app::error::{AppError, Result};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

/// 加密。返回可直接存进 BLOB 列的字节。
pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
    if plain.is_empty() {
        return Err(AppError::new("crypto", "待加密内容为空"));
    }
    dpapi(plain, true)
}

/// 解密。传入非本用户/非本机加密的数据会返回明确的 crypto 错误，不 panic。
pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    if blob.is_empty() {
        return Err(AppError::new("crypto", "待解密内容为空"));
    }
    dpapi(blob, false)
}

fn dpapi(input: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();

    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &in_blob,
                windows::core::PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        } else {
            CryptUnprotectData(
                &in_blob,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        }
    };

    // 无论成败都要释放，否则泄漏 LocalAlloc 出来的内存
    let _guard = BlobGuard(out_blob);
    ok.map_err(|e| {
        AppError::new(
            "crypto",
            if encrypt {
                "加密失败"
            } else {
                "解密失败（可能不是本机/本用户加密的数据）"
            },
        )
        .with_detail(e.to_string())
    })?;

    Ok(copied(&out_blob))
}

fn copied(blob: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() || blob.cbData == 0 {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec() }
}

struct BlobGuard(CRYPT_INTEGER_BLOB);

impl Drop for BlobGuard {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            unsafe {
                let _ = LocalFree(HLOCAL(self.0.pbData as *mut _));
            }
        }
    }
}

// ---------------------------------------------------------------- Chromium 信封

/// 从客户端的 `Local State` 取出 Chromium 主密钥。
///
/// 结构：`{"os_crypt": {"encrypted_key": base64("DPAPI" + DPAPI(32 字节密钥))}}`。
/// 前缀那 5 个字节是明文标记，必须先剥掉再 DPAPI 解密——直接解会失败，
/// 而失败信息只会说「参数错误」，看不出是前缀问题。
pub fn chromium_key(local_state: &std::path::Path) -> Result<aes_gcm::Key<aes_gcm::Aes256Gcm>> {
    let raw = std::fs::read(local_state)
        .map_err(|e| AppError::new("io", format!("读取 Local State 失败：{e}")))?;
    let json: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|e| AppError::new("invalid", format!("Local State 不是 JSON：{e}")))?;
    let encoded = json
        .pointer("/os_crypt/encrypted_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::new("invalid", "Local State 里没有 os_crypt.encrypted_key"))?;
    use base64::Engine;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim().as_bytes())
        .map_err(|e| AppError::new("invalid", format!("encrypted_key 不是合法 base64：{e}")))?;
    if blob.len() < 5 || &blob[..5] != b"DPAPI" {
        return Err(AppError::new("crypto", "encrypted_key 前缀不是 DPAPI"));
    }
    let key = unprotect(&blob[5..])?;
    if key.len() != 32 {
        return Err(AppError::new(
            "crypto",
            format!("Chromium 主密钥长度异常：{} 字节", key.len()),
        ));
    }
    Ok(*aes_gcm::Key::<aes_gcm::Aes256Gcm>::from_slice(&key))
}

/// 解开 Chromium 的 `v10` 信封：`"v10"` + 12 字节 nonce + 密文。
pub fn chromium_open(key: &aes_gcm::Key<aes_gcm::Aes256Gcm>, blob: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};
    if blob.len() < 15 || &blob[..3] != b"v10" {
        return Err(AppError::new("crypto", "不是 v10 信封"));
    }
    let cipher = aes_gcm::Aes256Gcm::new(key);
    let nonce = aes_gcm::Nonce::from_slice(&blob[3..15]);
    cipher
        .decrypt(nonce, &blob[15..])
        .map_err(|_| AppError::new("crypto", "AES-GCM 解密失败（密钥不匹配或密文被改）"))
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_unprotect_roundtrip() {
        let blob = protect(b"secret-token").unwrap();
        assert_ne!(blob, b"secret-token", "加密后不应等于原文");
        assert_eq!(unprotect(&blob).unwrap(), b"secret-token");
    }

    #[test]
    fn unprotect_garbage_returns_error_not_panic() {
        // 契约：非法密文返回明确错误，绝不 panic
        assert!(unprotect(b"not-a-dpapi-blob").is_err());
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(protect(b"").is_err());
        assert!(unprotect(b"").is_err());
    }

    #[test]
    fn chromium_envelope_roundtrip() {
        use aes_gcm::aead::{Aead, KeyInit};
        // 造一个 Chromium 主密钥（真实环境里它是 DPAPI 保护的，这里只测信封格式）
        let key = aes_gcm::Key::<aes_gcm::Aes256Gcm>::from_slice(&[7u8; 32]).to_owned();
        let cipher = aes_gcm::Aes256Gcm::new(&key);
        let nonce_bytes = [3u8; 12];
        let ciphertext = cipher
            .encrypt(
                aes_gcm::Nonce::from_slice(&nonce_bytes),
                br#"{"token":"tk"}"#.as_ref(),
            )
            .unwrap();

        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ciphertext);

        let plain = chromium_open(&key, &blob).unwrap();
        assert_eq!(plain, br#"{"token":"tk"}"#);
    }

    #[test]
    fn chromium_wrong_envelope_prefix_is_rejected() {
        let key = aes_gcm::Key::<aes_gcm::Aes256Gcm>::from_slice(&[0u8; 32]).to_owned();
        assert!(chromium_open(&key, b"v11-whatever").is_err());
        assert!(chromium_open(&key, b"v10short").is_err());
    }
}
