//! DPAPI 封装：账号凭据按账号单独加密（v1 是整个 JSON 一起加密）。
//!
//! 为什么改成按账号：读单个账号不必解开全部，且内存里同时存在的明文从
//! 「所有账号」降到「一个账号」。DPAPI 与当前 Windows 用户绑定，换用户/换机解不开。

use crate::app::error::{AppError, Result};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
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
        AppError::new("crypto", if encrypt { "加密失败" } else { "解密失败（可能不是本机/本用户加密的数据）" })
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
}
