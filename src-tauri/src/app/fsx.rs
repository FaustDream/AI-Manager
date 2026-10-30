//! 文件系统小工具。只做三件切换登录要用的事：**原子写**、**目录合并复制**、
//! **可删只读文件的删除**。
//!
//! 为什么值得单独一个文件：这三件事都踩过坑——
//!   - 直接 `fs::write` 客户端登录态，写到一半断电 = 登录态半截 JSON，客户端崩溃
//!   - `fs::remove_dir_all` 遇到只读文件在 Windows 上直接失败（客户端会留下只读的缓存）
//!   - 复制目录时目标已存在就整体删掉，会把其他账号的 `storage/user-<uid>-personal` 一起删了

use crate::app::error::{AppError, Result};
use std::path::{Path, PathBuf};

/// 先写同目录临时文件再 rename：读方永远看到完整内容或旧内容。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    if path.exists() {
        let _ = clear_readonly(path);
    }
    replace_file(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AppError::new("write_failed", format!("写入 {} 失败：{e}", path.display()))
    })
}

/// 用临时文件替换目标文件。
///
/// **不能用 `remove + rename`**：两步之间崩溃就会留下「目标不存在」，
/// 而目标可能是客户端唯一的一份登录票据。`MoveFileEx` 带替换标志，
/// 要么是旧内容、要么是新内容，不存在中间态。
#[cfg(windows)]
fn replace_file(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }
    let from = wide(tmp);
    let to = wide(dst);
    unsafe {
        MoveFileExW(
            PCWSTR(from.as_ptr()),
            PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::rename(tmp, dst)
}

/// 备份到指定目录，保留原文件名 + 时间戳；返回备份路径。
pub fn backup_to(src: &Path, dir: &Path, tag: &str) -> Result<PathBuf> {
    if !src.exists() {
        return Err(AppError::new("backup_failed", format!("待备份文件不存在：{}", src.display())));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::new("backup_failed", format!("创建备份目录失败：{e}")))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let target = dir.join(format!("{tag}-{stamp}-{name}"));
    std::fs::copy(src, &target)
        .map_err(|e| AppError::new("backup_failed", format!("备份 {} 失败：{e}", src.display())))?;
    Ok(target)
}

/// 合并复制目录：目标中已有的文件被覆盖，**目标里多出来的文件保留**。
///
/// WorkBuddy 的 `storage/` 下每个账号各有一份 `user-<uid>-personal`，
/// 整体替换会把别的账号的本地数据一起删掉——v1 就是这么丢过一次。
pub fn copy_dir_merge(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        let from = entry.path();
        if from.is_dir() {
            copy_dir_merge(&from, &to)?;
        } else {
            if to.exists() {
                let _ = clear_readonly(&to);
            }
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 整体替换目录（先删再拷），用于「登录态是全局的，必须换掉」的 `app/session`。
pub fn replace_dir(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        remove_path(dst)?;
    }
    copy_dir_merge(src, dst)
}

/// 删除文件或目录，先清只读位（Windows 上只读文件会让删除直接失败）。
pub fn remove_path(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let _ = clear_readonly(path);
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            remove_path(&entry.path())?;
        }
        std::fs::remove_dir(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn clear_readonly(path: &Path) -> std::io::Result<()> {
    let mut perms = std::fs::metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    if perms.readonly() {
        perms.set_readonly(false);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = tmp();
        let file = dir.path().join("a.info");
        write_atomic(&file, b"first").unwrap();
        write_atomic(&file, b"second").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"second");
        // 临时文件不能留在目录里
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "原子写不应留下临时文件");
    }

    #[test]
    fn merge_keeps_files_that_only_exist_in_target() {
        let dir = tmp();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::create_dir_all(dst.join("sub")).unwrap();
        std::fs::write(src.join("sub/new.txt"), b"new").unwrap();
        std::fs::write(dst.join("sub/keep.txt"), b"keep").unwrap();

        copy_dir_merge(&src, &dst).unwrap();

        assert_eq!(std::fs::read(dst.join("sub/new.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(dst.join("sub/keep.txt")).unwrap(), b"keep", "不能删掉目标独有文件");
    }

    #[test]
    fn replace_removes_stale_files() {
        let dir = tmp();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("only.txt"), b"x").unwrap();
        std::fs::write(dst.join("stale.txt"), b"y").unwrap();

        replace_dir(&src, &dst).unwrap();

        assert!(dst.join("only.txt").exists());
        assert!(!dst.join("stale.txt").exists(), "整体替换要清掉旧内容");
    }

    #[test]
    fn remove_handles_readonly_files() {
        let dir = tmp();
        let path = dir.path().join("ro.txt");
        std::fs::write(&path, b"x").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();

        remove_path(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn backup_lands_in_given_dir_with_tag() {
        let dir = tmp();
        let src = dir.path().join("auth.v1.dat");
        std::fs::write(&src, b"ticket").unwrap();
        let backup = backup_to(&src, &dir.path().join("backups"), "qoder").unwrap();
        assert!(backup.to_string_lossy().contains("backups"));
        assert!(backup.file_name().unwrap().to_string_lossy().starts_with("qoder-"));
        assert_eq!(std::fs::read(&backup).unwrap(), b"ticket");
    }
}
