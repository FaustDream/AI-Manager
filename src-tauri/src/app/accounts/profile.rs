//! 档案往返：把一份登录票据原样存下、原样写回。
//!
//! 契约 8a 的全部内容。看起来只有三行，但它是「切回账号不弹登录页」唯一可自动化的
//! 代理指标：客户端认的是票据里的 `sessionState`（服务端每次登录签发），
//! 任何一次「顺手重新格式化 JSON」都会让它失效，所以这里只做字节搬运，不解析。

use crate::contracts::ProfileStore;
use crate::app::error::{AppError, Result};
use crate::app::fsx;
use std::path::Path;

/// 单文件档案（`workbuddy-desktop.info` / `auth.v1.dat` 等）。
pub struct FileProfile;

impl ProfileStore for FileProfile {
    fn save(&self, src: &Path) -> Result<Vec<u8>> {
        std::fs::read(src).map_err(|e| {
            AppError::new("io", format!("读取登录票据失败：{}（{e}）", src.display()))
        })
    }

    /// 原子写：先写同目录临时文件再替换，客户端不会读到半截 JSON。
    fn restore(&self, blob: &[u8], dst: &Path) -> Result<()> {
        if blob.is_empty() {
            return Err(AppError::new("invalid", "档案为空，拒绝写回"));
        }
        fsx::write_atomic(dst, blob)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::ProfileStore;

    #[test]
    fn roundtrip_is_byte_identical() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("workbuddy-desktop.info");
        // 带 BOM、中文、CRLF：客户端自己写出来的文件就长这样，任何重编码都会改变字节
        let original = "\u{feff}{\"sessionState\":\"服务端签发\",\r\n\"uid\":\"u1\"}".as_bytes().to_vec();
        std::fs::write(&src, &original).unwrap();

        let blob = FileProfile.save(&src).unwrap();
        std::fs::write(&src, b"corrupted").unwrap();
        FileProfile.restore(&blob, &src).unwrap();

        assert_eq!(std::fs::read(&src).unwrap(), original, "契约 8a：必须按字节写回");
    }

    #[test]
    fn restore_replaces_whole_file_not_append() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("auth.info");
        std::fs::write(&path, b"old-content-that-is-longer").unwrap();
        FileProfile.restore(b"new", &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn missing_source_is_an_io_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = FileProfile.save(&dir.path().join("nope.info")).unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Io));
    }

    #[test]
    fn empty_blob_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("auth.info");
        std::fs::write(&path, b"keep-me").unwrap();
        assert!(FileProfile.restore(b"", &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep-me", "拒绝时不得改动原文件");
    }
}
