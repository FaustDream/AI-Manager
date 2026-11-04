//! 回收站：删除的东西先搬到 `data/trash/sessions/<tag>/`，按批次可恢复。
//!
//! 一个批次 = 一次删除动作 = 一个目录 + 一份 `manifest.json`。清单里记清楚
//! 「哪一条会话、什么类型、哪些文件、多少字节、库里删掉了哪些行」，
//! 恢复时照着搬回去——**没有清单就不算删成功**，这是这一层唯一的硬要求。

use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::paths;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    paths::data_dir().join("trash").join("sessions")
}

/// 同一秒内多次删除不能撞目录：撞了就加序号。
/// `create_dir` 的 AlreadyExists 错误做原子判定，不靠先 exists 再建
/// （并发删除请求落在同一秒时，exists 检查与创建之间会互相穿透）。
pub fn new_tag(root: &Path) -> String {
    let base = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut tag = base.clone();
    let mut index = 2;
    loop {
        match std::fs::create_dir(root.join(&tag)) {
            Ok(()) => return tag,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                tag = format!("{base}-{index}");
                index += 1;
            }
            Err(_) => return tag, // 建不了目录交给调用方在写清单时报错
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 清单条目。`kind` 决定恢复时用哪条逻辑。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub kind: String,
    pub source: String,
    pub session_id: String,
    pub title: String,
    /// 相对批次目录的路径（文件或目录）
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: i64,
    /// 从客户端库里删掉的原始行（JSON），恢复时原样写回
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub rows: Option<serde_json::Value>,
    pub account: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub tag: String,
    pub at: String,
    /// 这次删除涉及的客户端（恢复前要再关一次）
    #[serde(default)]
    pub platforms: Vec<String>,
    pub items: Vec<Item>,
}

impl Manifest {
    pub fn total_size(&self) -> i64 {
        self.items.iter().map(|i| i.size).sum()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} 条会话 · {:.1} MB",
            self.items.len(),
            self.total_size() as f64 / 1_048_576.0
        )
    }
}

pub fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("建回收站目录失败：{e}")))?;
    let text = serde_json::to_string_pretty(manifest)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("清单序列化失败：{e}")))?;
    crate::app::fsx::write_atomic(&dir.join("manifest.json"), text.as_bytes())
}

pub fn read_manifest(dir: &Path) -> Option<Manifest> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// 所有批次，最近的排前面。读不出清单的目录直接跳过。
pub fn list(root: &Path) -> Vec<Manifest> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<Manifest> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| read_manifest(&e.path()))
        .collect();
    out.sort_by(|a, b| b.tag.cmp(&a.tag));
    out
}

/// 彻底删掉一个批次，返回释放的字节数。
pub fn purge(root: &Path, tag: &str) -> Result<i64> {
    let dir = root.join(tag);
    if !dir.exists() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("没有这个批次：{tag}"),
        ));
    }
    let size = dir_size(&dir);
    crate::app::fsx::remove_path(&dir)?;
    Ok(size)
}

/// 把一个文件/目录搬进批次目录，返回相对路径。
///
/// `rename` 只在同一个盘内有效：数据目录在 D:、客户端在 C: 时它报 EXDEV
/// （os error 17），这时退回「复制 + 删源」——慢一点，但跨盘也能进回收站。
pub fn move_in(src: &Path, batch: &Path, rel: &str) -> Result<()> {
    let dest = batch.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("建目录失败：{e}")))?;
    }
    if dest.exists() {
        let _ = crate::app::fsx::remove_path(&dest);
    }
    if std::fs::rename(src, &dest).is_ok() {
        return Ok(());
    }
    crate::app::fsx::copy_any(src, &dest).map_err(|e| {
        AppError::new(
            ErrorKind::Io,
            format!("移动 {} 到回收站失败：{}", src.display(), e.message),
        )
    })?;
    crate::app::fsx::remove_path(src).map_err(|e| {
        AppError::new(
            ErrorKind::Io,
            format!("已复制到回收站，但源文件删不掉：{}", e.message),
        )
    })
}

/// 从批次目录搬回原位。跨盘时同样退回「复制 + 删源」。
pub fn move_out(batch: &Path, rel: &str, dest: &Path) -> Result<()> {
    let src = batch.join(rel);
    if !src.exists() {
        return Err(AppError::new(
            ErrorKind::Io,
            format!("回收站里找不到 {rel}"),
        ));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("建目录失败：{e}")))?;
    }
    if dest.exists() {
        let _ = crate::app::fsx::remove_path(dest);
    }
    if std::fs::rename(&src, dest).is_ok() {
        return Ok(());
    }
    crate::app::fsx::copy_any(&src, dest).map_err(|e| {
        AppError::new(
            ErrorKind::Io,
            format!("恢复 {} 失败：{}", dest.display(), e.message),
        )
    })?;
    crate::app::fsx::remove_path(&src).map_err(|e| {
        AppError::new(
            ErrorKind::Io,
            format!("已恢复，但回收站副本删不掉：{}", e.message),
        )
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let first = new_tag(dir.path());
        std::fs::create_dir_all(dir.path().join(&first)).unwrap();
        let second = new_tag(dir.path());
        assert_ne!(first, second);
        assert!(second.ends_with("-2"));
    }

    #[test]
    fn manifest_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            tag: "20260101-000000".into(),
            at: "2026-01-01T00:00:00+08:00".into(),
            platforms: vec!["workbuddy".into()],
            items: vec![Item {
                kind: "workbuddy".into(),
                source: "cn".into(),
                session_id: "s1".into(),
                title: "测试".into(),
                files: vec!["cn/s1.jsonl".into()],
                size: 12,
                rows: None,
                account: "u1".into(),
            }],
        };
        write_manifest(dir.path(), &manifest).unwrap();
        let back = read_manifest(dir.path()).unwrap();
        assert_eq!(back.items.len(), 1);
        assert_eq!(back.total_size(), 12);
    }
}
