//! 维护页的数据：空间占用、回收站清空。
//!
//! 只做「量得出来、删得掉」两件事，不引入新的业务逻辑。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::fsx::dir_size;
use crate::app::paths;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct SpaceEntry {
    pub name: String,
    pub path: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct SpaceReport {
    pub entries: Vec<SpaceEntry>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: i64,
}

/// `data/` 下各子目录 + 几个大文件的体积。清理前先知道钱花在哪。
pub fn space() -> SpaceReport {
    space_in(&paths::data_dir())
}

fn space_in(root: &Path) -> SpaceReport {
    let mut entries = Vec::new();
    let mut total = 0;

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(items) = std::fs::read_dir(root) {
        for item in items.flatten() {
            if item.path().is_dir() {
                dirs.push(item.path());
            }
        }
    }
    dirs.sort();

    for dir in dirs {
        let size = dir_size(&dir);
        total += size;
        entries.push(SpaceEntry {
            name: dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            path: dir.display().to_string(),
            size,
        });
    }
    for name in ["app.db", "app.db-wal", "app.db-shm"] {
        let path = root.join(name);
        if let Ok(meta) = std::fs::metadata(&path) {
            total += meta.len() as i64;
            entries.push(SpaceEntry {
                name: name.to_string(),
                path: path.display().to_string(),
                size: meta.len() as i64,
            });
        }
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.size));
    SpaceReport { entries, total }
}

/// 回收站里的批次：所有 `data/trash/**/<目录名>` 的一级子目录。
/// `sessions/<tag>` 与 `platform-skills-<平台>/<时间戳>` 都算批次。
pub fn trash_batches() -> Vec<SpaceEntry> {
    trash_batches_in(&paths::data_dir().join("trash"))
}

fn trash_batches_in(root: &Path) -> Vec<SpaceEntry> {
    let mut out = Vec::new();
    let Ok(groups) = std::fs::read_dir(root) else {
        return out;
    };
    for group in groups.flatten() {
        if !group.path().is_dir() {
            continue;
        }
        // 平台技能回收站下还有一层时间戳，按批次展开
        let mut inner: Vec<PathBuf> = Vec::new();
        if let Ok(children) = std::fs::read_dir(group.path()) {
            for child in children.flatten() {
                if child.path().is_dir() {
                    inner.push(child.path());
                }
            }
        }
        for batch in inner {
            out.push(SpaceEntry {
                name: format!(
                    "{}/{}",
                    group.file_name().to_string_lossy(),
                    batch
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                size: dir_size(&batch),
                path: batch.display().to_string(),
            });
        }
    }
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

/// 清空回收站：`tag=None` 全清，给了就只清名字里包含该 tag 的批次。
pub fn purge_trash(tag: Option<&str>) -> Result<i64> {
    purge_trash_in(&paths::data_dir().join("trash"), tag)
}

fn purge_trash_in(root: &Path, tag: Option<&str>) -> Result<i64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut freed = 0i64;
    let groups: Vec<PathBuf> = match std::fs::read_dir(root) {
        Ok(items) => items
            .flatten()
            .map(|i| i.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) => return Ok(0),
    };
    for group in groups {
        let batches: Vec<PathBuf> = match std::fs::read_dir(&group) {
            Ok(items) => items
                .flatten()
                .map(|i| i.path())
                .filter(|p| p.is_dir())
                .collect(),
            Err(_) => continue,
        };
        for batch in batches {
            let name = batch
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(want) = tag {
                if !name.contains(want) {
                    continue;
                }
            }
            freed += dir_size(&batch);
            if crate::app::fsx::remove_path(&batch).is_err() {
                return Err(AppError::new(
                    ErrorKind::Io,
                    format!("清不掉 {}", batch.display()),
                ));
            }
        }
        // 组目录空了就一并删掉，不留空壳
        if std::fs::read_dir(&group)
            .map(|mut i| i.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&group);
        }
    }
    Ok(freed)
}

/// 打开数据目录（维护页的「打开」按钮）。
pub fn open_data_dir() -> Result<()> {
    let dir = paths::data_dir();
    crate::app::process::spawn_with_args(
        &std::path::PathBuf::from("explorer.exe"),
        &[dir.to_string_lossy().as_ref()],
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 直接断言显式路径，不碰 `paths::set_data_dir_for_test`——
    /// 那是进程级 OnceLock，并行测试会互相污染。
    #[test]
    fn space_report_and_purge_trash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("backups")).unwrap();
        std::fs::write(dir.path().join("backups").join("a.bin"), vec![0u8; 4096]).unwrap();
        std::fs::write(dir.path().join("app.db"), vec![0u8; 1024]).unwrap();
        let trash = dir.path().join("trash").join("sessions");
        std::fs::create_dir_all(trash.join("20260101")).unwrap();
        std::fs::write(trash.join("20260101").join("f.bin"), vec![0u8; 2048]).unwrap();
        std::fs::create_dir_all(trash.join("20260102")).unwrap();

        let report = space_in(dir.path());
        assert!(report.total >= 5120);
        assert!(report.entries.iter().any(|e| e.name == "backups"));
        assert_eq!(trash_batches_in(&dir.path().join("trash")).len(), 2);

        let freed = purge_trash_in(&dir.path().join("trash"), Some("20260101")).unwrap();
        assert!(freed >= 2048);
        assert!(!trash.join("20260101").exists());
        assert!(trash.join("20260102").exists(), "不匹配的批次不能动");
    }
}
