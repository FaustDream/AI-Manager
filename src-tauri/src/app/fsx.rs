//! 文件系统小工具。只做三件切换登录要用的事：**原子写**、**目录合并复制**、
//! **可删只读文件的删除**。
//!
//! 为什么值得单独一个文件：这三件事都踩过坑——
//!   - 直接 `fs::write` 客户端登录态，写到一半断电 = 登录态半截 JSON，客户端崩溃
//!   - `fs::remove_dir_all` 遇到只读文件在 Windows 上直接失败（客户端会留下只读的缓存）
//!   - 复制目录时目标已存在就整体删掉，会把其他账号的 `storage/user-<uid>-personal` 一起删了

use crate::app::error::{AppError, ErrorKind, Result};
use std::path::{Path, PathBuf};

/// 先写同目录临时文件再 rename：读方永远看到完整内容或旧内容。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // tmp 名带原子递增序号：同进程两个线程写同一路径不能互相踩
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp-{}-{seq}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    // rename 前把数据刷盘：MOVEFILE_WRITE_THROUGH 只保证改名元数据落盘，
    // 不保证 tmp 的内容已经在磁盘上——掉电时会以新文件名留下半截内容
    let _ = std::fs::OpenOptions::new()
        .write(true)
        .open(&tmp)
        .and_then(|file| file.sync_all());
    if path.exists() {
        let _ = clear_readonly(path);
    }
    replace_file(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AppError::new(
            ErrorKind::WriteFailed,
            format!("写入 {} 失败：{e}", path.display()),
        )
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
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
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

/// 文件或目录的占用字节数。目录递归累加，读不到的部分按 0 计。
pub fn dir_size(path: &Path) -> i64 {
    let Ok(stat) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if stat.is_file() {
        return stat.len() as i64;
    }
    if !stat.is_dir() {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            total += dir_size(&entry.path());
        }
    }
    total
}

/// Chromium profile 里按定义可重建的纯缓存目录。
/// 登录判定只认 Cookies / Local Storage / 票据，缓存不参与；但整目录拷进每个账号档案、
/// 每份退出登录备份后，几百 MB 会被复制到多处，客户端启动时自己就会重建。
pub const CHROMIUM_CACHE_DIRS: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnWebGPUCache",
    "DawnGraphiteCache",
    "GrShaderCache",
    "ShaderCache",
];

/// 备份批次保留份数：写前备份是唯一回退依据，但不能无限堆积。
pub const BACKUP_KEEP: usize = 10;

/// 复制登录态目录，跳过纯缓存；其余语义与 `copy_dir_merge` 一致。
/// 嵌套在 `Partitions/` 下的同名缓存目录也跳过。
pub fn copy_profile_skipping_caches(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if from.is_dir() && CHROMIUM_CACHE_DIRS.contains(&name.as_str()) {
            continue;
        }
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_profile_skipping_caches(&from, &to)?;
        } else {
            if to.exists() {
                let _ = clear_readonly(&to);
            }
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 备份目录只留最近 keep 个条目，按修改时间从旧到新删。
///
/// 只允许传「条目全部由本工具按批次新建」的目录：`session-restore/` 混有 v1 遗留、
/// `manual-*` / `switch-exp-*` 是人工快照，按 CORE-01 一律不传进来。
pub fn prune_children(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<(Option<std::time::SystemTime>, PathBuf)> = entries
        .flatten()
        .map(|e| {
            let at = e.metadata().and_then(|m| m.modified()).ok();
            (at, e.path())
        })
        .collect();
    // 最新在前；同一时刻创建的按名字兜底，保证删除顺序稳定
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, path) in items.iter().skip(keep) {
        // 新备份已经落盘才走到这里，旧批次清不掉不该让业务失败，下次再试
        let _ = remove_path(path);
    }
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

/// 复制文件或目录（目录递归）。
///
/// 存在的理由：`std::fs::rename` **只能同一个盘**。数据目录在 D:、客户端在 C: 时
/// 它直接报 EXDEV（os error 17），回收站就永远搬不进去——「彻底删除 0 条」就是这么来的。
pub fn copy_any(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        return copy_dir_merge(src, dst);
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if dst.exists() {
        let _ = clear_readonly(dst);
    }
    std::fs::copy(src, dst)?;
    Ok(())
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

    /// 跨盘搬移靠它兜底：文件和目录都要能复制。
    #[test]
    fn copy_any_handles_files_and_dirs() {
        let dir = tmp();
        let file = dir.path().join("a.jsonl");
        std::fs::write(&file, b"body").unwrap();
        copy_any(&file, &dir.path().join("out/a.jsonl")).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("out/a.jsonl")).unwrap(),
            b"body"
        );

        let tree = dir.path().join("tree/sub");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("x.log"), b"log").unwrap();
        copy_any(&dir.path().join("tree"), &dir.path().join("moved")).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("moved/sub/x.log")).unwrap(),
            b"log"
        );
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
        assert_eq!(
            std::fs::read(dst.join("sub/keep.txt")).unwrap(),
            b"keep",
            "不能删掉目标独有文件"
        );
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
    fn profile_copy_skips_only_chromium_caches() {
        let dir = tmp();
        let src = dir.path().join("session");
        let dst = dir.path().join("pack");
        std::fs::create_dir_all(src.join("Cache")).unwrap();
        std::fs::create_dir_all(src.join("Code Cache")).unwrap();
        std::fs::create_dir_all(src.join("GPUCache")).unwrap();
        std::fs::create_dir_all(src.join("Partitions/sub/Cache")).unwrap();
        std::fs::create_dir_all(src.join("Local Storage")).unwrap();
        std::fs::write(src.join("Cache/data_1"), vec![0u8; 2048]).unwrap();
        std::fs::write(src.join("Code Cache/js"), b"js").unwrap();
        std::fs::write(src.join("GPUCache/gpu"), b"gpu").unwrap();
        std::fs::write(src.join("Partitions/sub/Cache/blob"), b"blob").unwrap();
        std::fs::write(src.join("Partitions/sub/Preferences"), b"prefs").unwrap();
        std::fs::write(src.join("Cookies"), b"cookies").unwrap();
        std::fs::write(src.join("Local Storage/leveldb"), b"tokens").unwrap();
        std::fs::write(src.join("Local State"), b"state").unwrap();

        copy_profile_skipping_caches(&src, &dst).unwrap();

        assert!(!dst.join("Cache").exists());
        assert!(!dst.join("Code Cache").exists());
        assert!(!dst.join("GPUCache").exists());
        assert!(
            !dst.join("Partitions/sub/Cache").exists(),
            "嵌套缓存也要跳过"
        );
        assert_eq!(std::fs::read(dst.join("Cookies")).unwrap(), b"cookies");
        assert_eq!(
            std::fs::read(dst.join("Local Storage/leveldb")).unwrap(),
            b"tokens"
        );
        assert_eq!(std::fs::read(dst.join("Local State")).unwrap(), b"state");
        assert_eq!(
            std::fs::read(dst.join("Partitions/sub/Preferences")).unwrap(),
            b"prefs"
        );
    }

    #[test]
    fn prune_children_keeps_only_latest_batches() {
        let dir = tmp();
        let base = dir.path().join("backups");
        std::fs::create_dir_all(&base).unwrap();
        for index in 0..12 {
            let batch = base.join(format!("202601{:03}", index));
            std::fs::create_dir_all(&batch).unwrap();
            std::fs::write(batch.join("f.bin"), b"x").unwrap();
        }

        prune_children(&base, BACKUP_KEEP);

        assert_eq!(std::fs::read_dir(&base).unwrap().count(), BACKUP_KEEP);
        assert!(!base.join("202601000").exists(), "最旧的批次要被回收");
        assert!(base.join("202601011").exists(), "最新的批次必须留着");
    }
}
