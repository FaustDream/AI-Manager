//! 技能库：`data/skills/<名字>/` 里放一份，部署就是复制到各平台目录。
//!
//! 归属判定只有一条：**库里有同名技能 = 这是我们部署的**。
//! 平台目录里有、库里没有的，一律当用户自己加的，不覆盖、不删除。
//! （设计稿原计划在 `hub_items` 里记 `deployed`，实际不需要——省掉一层同步。）

use crate::app::error::{AppError, Result};
use crate::app::hub::targets;
use crate::app::hub::{DeployReport, Hub};
use crate::contracts::TargetKey;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 单个技能最多读这么多文件参与摘要：正常技能只有几十个文件，防的是误放的大目录。
const MAX_FILES: usize = 2000;

#[derive(Debug, Clone, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub files: usize,
    pub size: i64,
    pub digest: String,
    pub updated_at: i64,
    pub valid: bool,
    /// 无效原因（缺 SKILL.md 之类）
    pub reason: String,
}

impl SkillInfo {
    pub fn summary(&self) -> String {
        format!(
            "{} · {} 个文件 · {:.1} KB",
            self.name,
            self.files,
            self.size as f64 / 1024.0
        )
    }
}

pub fn library(hub: &Hub) -> PathBuf {
    data_dir(hub).join("skills")
}

fn data_dir(hub: &Hub) -> PathBuf {
    hub.data_dir()
}

/// 扫库：只认目录，且必须有 SKILL.md。
pub fn scan(hub: &Hub) -> Vec<SkillInfo> {
    let root = library(hub);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<SkillInfo> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| read_skill(&entry.path()))
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

pub fn find(hub: &Hub, name: &str) -> Option<SkillInfo> {
    let path = library(hub).join(name);
    path.is_dir().then(|| read_skill(&path))
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct InstalledSkill {
    pub name: String,
    /// 在技能库里：state 是与库对账的结果（synced/stale）；
    /// 不在库里（平台自带或用户手工放置）：state = foreign
    pub in_library: bool,
    pub state: String,
}

/// 平台技能目录里**实际存在**的技能（平台页按这个展示，不列整个库）。
pub fn installed(hub: &Hub, key: TargetKey) -> Vec<InstalledSkill> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(hub.skill_dir(key)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join("SKILL.md").is_file() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        match find(hub, &name) {
            Some(skill) => {
                let state = if digest_dir_cached(&path) == skill.digest {
                    "synced"
                } else {
                    "stale"
                };
                out.push(InstalledSkill {
                    name,
                    in_library: true,
                    state: state.into(),
                });
            }
            None => out.push(InstalledSkill {
                name,
                in_library: false,
                state: "foreign".into(),
            }),
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

pub fn read_skill(folder: &Path) -> SkillInfo {
    let name = folder
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let updated_at = std::fs::metadata(folder)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let entry = folder.join("SKILL.md");
    if !entry.is_file() {
        return SkillInfo {
            name,
            description: String::new(),
            files: 0,
            size: 0,
            digest: String::new(),
            updated_at,
            valid: false,
            reason: "缺少 SKILL.md".into(),
        };
    }

    let text = std::fs::read_to_string(&entry).unwrap_or_default();
    let (meta_name, description) = frontmatter(&text);
    let (files, size) = walk_stats(folder);
    SkillInfo {
        name: if meta_name.is_empty() {
            name
        } else {
            meta_name
        },
        description,
        files,
        size,
        digest: digest_dir(folder),
        updated_at,
        valid: true,
        reason: String::new(),
    }
}

/// SKILL.md 的 frontmatter：`---` 之间 `key: value`，只取 name 与 description。
fn frontmatter(text: &str) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (name, description);
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        match key.trim() {
            "name" => name = value,
            "description" => description = value,
            _ => {}
        }
    }
    (name, description)
}

fn walk_stats(folder: &Path) -> (usize, i64) {
    let mut files = 0usize;
    let mut size = 0i64;
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(_) => {
                    files += 1;
                    size += entry.metadata().map(|m| m.len() as i64).unwrap_or(0);
                }
                Err(_) => {}
            }
            if files > MAX_FILES {
                return (files, size);
            }
        }
    }
    (files, size)
}

/// 目录摘要：路径 + 长度 + 内容一起进 SHA-256。用来判断「平台上的副本还是不是库里的那份」。
pub fn digest_dir(folder: &Path) -> String {
    let mut items: Vec<(String, PathBuf)> = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(_) => {
                    let rel = path
                        .strip_prefix(folder)
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    items.push((rel, path));
                }
                Err(_) => {}
            }
        }
        if items.len() > MAX_FILES {
            break;
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, path) in items {
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        if let Ok(bytes) = std::fs::read(&path) {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
        hasher.update([0u8]);
    }
    format!("{:x}", hasher.finalize())
}

/// 目录摘要的进程内缓存：轻量指纹没变就直接复用上次的 SHA-256。
/// 快照每次刷新对全部技能 × 平台各算一遍，全量读文件内容太贵；
/// 指纹含 mtime，内容一变指纹就变，不会拿旧摘要冒充新状态。
fn digest_dir_cached(folder: &Path) -> String {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (u64, String)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let fp = fingerprint(folder);
    if let Ok(guard) = cache.lock() {
        if let Some((seen, digest)) = guard.get(folder) {
            if *seen == fp {
                return digest.clone();
            }
        }
    }
    let digest = digest_dir(folder);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(folder.to_path_buf(), (fp, digest.clone()));
    }
    digest
}

/// 廉价指纹：不读文件内容，只哈希「相对路径 + 大小 + mtime」。
fn fingerprint(folder: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut items: Vec<(String, u64, u64)> = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    'outer: while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(_) => {
                    let Ok(meta) = entry.metadata() else { continue };
                    let rel = path
                        .strip_prefix(folder)
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    items.push((rel, meta.len(), mtime));
                }
                Err(_) => {}
            }
            if items.len() > MAX_FILES {
                break 'outer;
            }
        }
    }
    items.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for item in &items {
        item.hash(&mut hasher);
    }
    hasher.finish()
}

/// 每个平台的状态：`synced` / `stale` / `missing`。
pub fn states(hub: &Hub, skill: &SkillInfo) -> Vec<(TargetKey, &'static str)> {
    targets::TARGETS
        .iter()
        .map(|target| {
            let dest = hub.skill_dir(target.key).join(&skill.name);
            if !dest.exists() {
                (target.key, "missing")
            } else if digest_dir_cached(&dest) == skill.digest {
                (target.key, "synced")
            } else {
                (target.key, "stale")
            }
        })
        .collect()
}

/// 部署到指定平台。内容一样的平台直接跳过，内容不同的报冲突（除非 overwrite）。
pub fn deploy(hub: &Hub, name: &str, keys: &[TargetKey], overwrite: bool) -> Result<DeployReport> {
    let skill =
        find(hub, name).ok_or_else(|| AppError::new("invalid", format!("库里没有技能 {name}")))?;
    if !skill.valid {
        return Err(AppError::new(
            "invalid",
            format!("技能 {name} 无效：{}", skill.reason),
        ));
    }
    let digest = if skill.digest.is_empty() {
        digest_dir(&library(hub).join(name))
    } else {
        skill.digest
    };
    let source = library(hub).join(name);
    let mut report = DeployReport::default();

    for key in keys {
        let dest = hub.skill_dir(*key).join(name);
        if dest.exists() {
            let same = digest_dir_cached(&dest) == digest;
            if same {
                report.push(true, targets::key_name(*key), "已是最新", &dest, false);
                continue;
            }
            if !overwrite {
                report.push(
                    false,
                    targets::key_name(*key),
                    "平台已有同名技能，未覆盖",
                    &dest,
                    true,
                );
                continue;
            }
            // 覆盖前先挪走旧副本：直接被占用时删不掉，复制会写一半
            if crate::app::fsx::remove_path(&dest).is_err() {
                report.push(
                    false,
                    targets::key_name(*key),
                    "旧副本删不掉（客户端可能在用）",
                    &dest,
                    false,
                );
                continue;
            }
        }
        match copy_tree(&source, &dest) {
            Ok(()) => report.push(true, targets::key_name(*key), "已同步", &dest, false),
            Err(e) => report.push(false, targets::key_name(*key), e.message, &dest, false),
        }
    }
    Ok(report)
}

/// 启动自动同步（v1 的 `auto_sync`）：把「本工具部署过、但内容落后于库」的
/// 平台副本按库版本刷新。
///
/// 与 v1 的差别：v1 按归属表连缺失的平台一起铺，v2 没有归属表——
/// 「库里有」不等于「应该到处都在」，**missing 不自动铺**，部署到哪些平台
/// 仍由用户在界面里显式选。MCP 不参与：v2 没有中心 MCP 库，无法重建
/// 「这条应该在哪」；真要做数据源是 v1 导入进 `hub_items` 的归属表。
pub fn sync_all(hub: &Hub) -> DeployReport {
    let mut report = DeployReport::default();
    for skill in scan(hub).into_iter().filter(|s| s.valid) {
        for (key, state) in states(hub, &skill) {
            if state != "stale" {
                continue;
            }
            let one = deploy(hub, &skill.name, &[key], true).unwrap_or_default();
            report.steps.extend(one.steps);
        }
    }
    report
}

/// 从平台移除。只动库里有同名技能的副本（= 我们部署的）。
pub fn remove(hub: &Hub, name: &str, keys: &[TargetKey]) -> DeployReport {
    let ours = library(hub).join(name).is_dir();
    let mut report = DeployReport::default();
    for key in keys {
        let dest = hub.skill_dir(*key).join(name);
        if !ours {
            report.push(
                false,
                targets::key_name(*key),
                "库里没有这个技能，不敢删",
                &dest,
                false,
            );
            continue;
        }
        if !dest.exists() {
            report.push(true, targets::key_name(*key), "本来就没有", &dest, false);
            continue;
        }
        // 先进回收站再删：跨盘符不能 rename，用「复制 + 删除」，
        // 复制失败就直接放弃，绝不在半路上把平台上的副本删掉
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let saved = crate::app::paths::data_dir()
            .join("trash")
            .join(format!("platform-skills-{}", targets::key_name(*key)))
            .join(&stamp)
            .join(name);
        match copy_tree(&dest, &saved) {
            Ok(()) => match crate::app::fsx::remove_path(&dest) {
                Ok(()) => report.push(
                    true,
                    targets::key_name(*key),
                    format!("已移到回收站：{}", saved.display()),
                    &dest,
                    false,
                ),
                // 回收站副本已在，但平台上的副本没删掉：必须如实报失败，
                // 否则用户以为删干净了，下次同步又会把它部署回去
                Err(e) => report.push(
                    false,
                    targets::key_name(*key),
                    format!(
                        "平台目录删除失败（副本已存回收站 {}）：{}",
                        saved.display(),
                        e.message
                    ),
                    &dest,
                    false,
                ),
            },
            Err(e) => report.push(
                false,
                targets::key_name(*key),
                format!("备份到回收站失败：{}", e.message),
                &dest,
                false,
            ),
        }
    }
    report
}

/// 从平台导入一份技能进库（库里已有同名则报错，不覆盖）。
pub fn import(hub: &Hub, key: TargetKey, name: &str) -> Result<SkillInfo> {
    let source = hub.skill_dir(key).join(name);
    if !source.is_dir() {
        return Err(AppError::new(
            "invalid",
            format!("{} 上没有技能 {name}", targets::label(key)),
        ));
    }
    if !source.join("SKILL.md").is_file() {
        return Err(AppError::new(
            "invalid",
            format!("{name} 没有 SKILL.md，不像是一个技能"),
        ));
    }
    let dest = library(hub).join(name);
    if dest.exists() {
        return Err(AppError::new("conflict", format!("库里已经有 {name}")));
    }
    std::fs::create_dir_all(library(hub))
        .map_err(|e| AppError::new("io", format!("建技能库失败：{e}")))?;
    copy_tree(&source, &dest)?;
    Ok(read_skill(&dest))
}

/// 各平台 `skills/` 下现有的技能名，供「从平台导入」挑。
pub fn platform_skills(hub: &Hub) -> Vec<(TargetKey, String)> {
    let mut out = Vec::new();
    for target in targets::TARGETS {
        let Ok(entries) = std::fs::read_dir(hub.skill_dir(target.key)) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() && entry.path().join("SKILL.md").is_file() {
                out.push((target.key, entry.file_name().to_string_lossy().to_string()));
            }
        }
    }
    out
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| AppError::new("io", format!("建目录失败：{e}")))?;
    let entries =
        std::fs::read_dir(src).map_err(|e| AppError::new("io", format!("读技能目录失败：{e}")))?;
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|e| AppError::new("io", format!("复制 {} 失败：{e}", from.display())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(dir: &Path) -> Hub {
        Hub::at(dir.join("home"), dir.join("appdata"), dir.join("data"))
    }

    fn seed_skill(hub: &Hub, name: &str, body: &str) -> PathBuf {
        let path = library(hub).join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: 测试用\n---\n\n{body}\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn scan_reads_frontmatter_and_flags_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        seed_skill(&hub, "pdf-reader", "读 PDF");
        std::fs::create_dir_all(library(&hub).join("broken")).unwrap();

        let list = scan(&hub);
        assert_eq!(list.len(), 2);
        let ok = list.iter().find(|s| s.name == "pdf-reader").unwrap();
        assert!(ok.valid);
        assert_eq!(ok.description, "测试用");
        assert!(ok.files >= 1);
        assert!(!ok.digest.is_empty());
        let broken = list.iter().find(|s| !s.valid).unwrap();
        assert!(broken.reason.contains("SKILL.md"));
    }

    #[test]
    fn deploy_then_conflict_then_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        seed_skill(&hub, "pdf-reader", "v1");

        let first = deploy(&hub, "pdf-reader", &[TargetKey::QoderCn], false).unwrap();
        assert_eq!(first.done(), 1);
        let dest = hub.skill_dir(TargetKey::QoderCn).join("pdf-reader");
        assert!(dest.join("SKILL.md").is_file());

        // 已是最新：重复部署不复制，也不报冲突
        let again = deploy(&hub, "pdf-reader", &[TargetKey::QoderCn], false).unwrap();
        assert_eq!(again.done(), 1);
        assert!(again.conflicts().is_empty());
        assert!(again.steps[0].message.contains("已是最新"));

        // 平台侧被改过 → 不覆盖，报冲突，内容保持原样
        std::fs::write(dest.join("SKILL.md"), "外面改的").unwrap();
        let conflict = deploy(&hub, "pdf-reader", &[TargetKey::QoderCn], false).unwrap();
        assert_eq!(conflict.done(), 0);
        assert_eq!(conflict.conflicts(), vec!["qoder_cn"]);
        assert_eq!(
            std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "外面改的"
        );

        // 用户确认后覆盖
        let forced = deploy(&hub, "pdf-reader", &[TargetKey::QoderCn], true).unwrap();
        assert_eq!(forced.done(), 1);
        assert!(std::fs::read_to_string(dest.join("SKILL.md"))
            .unwrap()
            .contains("v1"));
    }

    #[test]
    fn remove_only_touches_skills_in_library() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        seed_skill(&hub, "pdf-reader", "v1");
        deploy(&hub, "pdf-reader", &[TargetKey::WorkBuddy], false).unwrap();

        let report = remove(&hub, "pdf-reader", &[TargetKey::WorkBuddy]);
        assert_eq!(report.done(), 1);
        assert!(!hub
            .skill_dir(TargetKey::WorkBuddy)
            .join("pdf-reader")
            .exists());

        // 库里没有的技能：一个字节都不动
        let foreign = hub.skill_dir(TargetKey::TraeCn).join("别人的技能");
        std::fs::create_dir_all(&foreign).unwrap();
        let report = remove(&hub, "别人的技能", &[TargetKey::TraeCn]);
        assert_eq!(report.done(), 0);
        assert!(foreign.exists(), "不是我们部署的不能删");
    }

    #[test]
    fn sync_all_refreshes_stale_but_never_deploys_missing() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        seed_skill(&hub, "pdf-reader", "v1");
        deploy(&hub, "pdf-reader", &[TargetKey::QoderCn], false).unwrap();

        // 库更新 → 自动同步把已部署的平台副本刷成库版本
        std::fs::write(
            library(&hub).join("pdf-reader").join("SKILL.md"),
            "---\nname: pdf-reader\ndescription: 测试用\n---\n\nv2\n",
        )
        .unwrap();
        let report = sync_all(&hub);
        assert_eq!(report.done(), 1, "只刷新已部署的那一个平台");
        assert!(std::fs::read_to_string(
            hub.skill_dir(TargetKey::QoderCn)
                .join("pdf-reader")
                .join("SKILL.md")
        )
        .unwrap()
        .contains("v2"));

        // missing 的平台不会被自动铺——部署目标是用户显式选的
        assert!(!hub.skill_dir(TargetKey::ZCode).join("pdf-reader").exists());

        // 平台自带、库里没有的技能：同步根本看不见它
        let foreign = hub.skill_dir(TargetKey::TraeCn).join("别人的技能");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("SKILL.md"), "---\nname: 别人的技能\n---\n").unwrap();
        let again = sync_all(&hub);
        assert!(again
            .steps
            .iter()
            .all(|s| !s.message.contains("别人的技能")));
    }

    #[test]
    fn import_refuses_to_overwrite_library_copy() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        let from_platform = hub.skill_dir(TargetKey::ZCode).join("web-search");
        std::fs::create_dir_all(&from_platform).unwrap();
        std::fs::write(
            from_platform.join("SKILL.md"),
            "---\nname: web-search\n---\n",
        )
        .unwrap();

        let imported = import(&hub, TargetKey::ZCode, "web-search").unwrap();
        assert_eq!(imported.name, "web-search");
        assert!(library(&hub).join("web-search").join("SKILL.md").exists());

        let again = import(&hub, TargetKey::ZCode, "web-search").unwrap_err();
        assert_eq!(again.message, "库里已经有 web-search");
    }
}
