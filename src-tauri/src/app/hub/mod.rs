//! 技能与 MCP 的目标平台（P5）。
//!
//! 这一层只做两件事：**路径**（`targets.rs` 的常量表）与 **MCP 配置的读写**
//! （`mcp.rs` 的格式转换）。技能库与部署在 `skills.rs`。
//!
//! 不建数据库表：「这条是不是本工具写的」靠写进配置里的 `_aimanager` 标记判断，
//! 比在本地库记一份 deployed 更经得起用户手动改配置。代价是别人的配置里
//! 不会有这个标记，正好符合契约 16 的诉求。

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub ok: bool,
    pub target: String,
    pub message: String,
    pub path: String,
    /// 目标已有同名但内容不同：需要用户确认后才能覆盖
    pub conflict: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeployReport {
    pub steps: Vec<Step>,
}

impl DeployReport {
    fn push(
        &mut self,
        ok: bool,
        target: &str,
        message: impl Into<String>,
        path: &Path,
        conflict: bool,
    ) {
        self.steps.push(Step {
            ok,
            target: target.to_string(),
            message: message.into(),
            path: path.display().to_string(),
            conflict,
        });
    }

    pub fn done(&self) -> usize {
        self.steps.iter().filter(|s| s.ok).count()
    }

    pub fn conflicts(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter(|s| s.conflict)
            .map(|s| s.target.as_str())
            .collect()
    }

    pub fn summary(&self) -> String {
        let mut text = format!("{} 个平台成功", self.done());
        let conflicts = self.conflicts();
        if !conflicts.is_empty() {
            text.push_str(&format!(
                "，{} 个冲突（{}）",
                conflicts.len(),
                conflicts.join("、")
            ));
        }
        text
    }
}

pub mod mcp;
pub mod skills;
pub mod targets;

use crate::app::error::{AppError, Result};
use crate::app::paths;
use crate::contracts::{HubStore, McpEntry, TargetKey};
use chrono::Local;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub struct Hub {
    home: PathBuf,
    appdata: PathBuf,
    data: PathBuf,
    /// v1 管过的 MCP 名字。v1 写配置时不带 `_aimanager` 标记，
    /// 光看标记会把用户已有的条目全判成「别人写的」，那就一条都管不了。
    legacy_mcp: std::collections::HashSet<String>,
}

impl Hub {
    pub fn system() -> Self {
        Self::at(paths::home(), appdata_dir(), paths::data_dir())
    }

    pub fn at(home: PathBuf, appdata: PathBuf, data: PathBuf) -> Self {
        let legacy_mcp = read_legacy_mcp(&data);
        Self {
            home,
            appdata,
            data,
            legacy_mcp,
        }
    }

    /// 这条 MCP 算不算「本工具的」：要么带标记，要么在 v1 的 hub.json 里。
    pub fn owns_mcp(&self, name: &str, body: &serde_json::Value) -> bool {
        mcp::written_by_us(body) || self.legacy_mcp.contains(name)
    }

    pub fn skill_dir(&self, key: TargetKey) -> PathBuf {
        targets::skill_dir(&self.home, key)
    }

    pub fn mcp_path(&self, key: TargetKey) -> PathBuf {
        targets::mcp_file(&self.home, &self.appdata, key)
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data.join("backups").join("hub")
    }

    /// 数据目录（技能库与回收站都在它下面）。
    pub fn data_dir(&self) -> PathBuf {
        self.data.clone()
    }

    /// 技能库：`data/skills/`。
    pub fn skills(&self) -> Vec<crate::app::hub::skills::SkillInfo> {
        skills::scan(self)
    }
}

/// v1 的 `data/hub.json` 里 `mcp` 的键：那批条目是 v1 写进去的，归我们管。
fn read_legacy_mcp(data: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Ok(text) = std::fs::read_to_string(data.join("hub.json")) else {
        return out;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return out;
    };
    if let Some(map) = value.get("mcp").and_then(|v| v.as_object()) {
        out.extend(map.keys().cloned());
    }
    out
}

fn appdata_dir() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| paths::home().join("AppData").join("Roaming"))
}

impl HubStore for Hub {
    fn target_dir(&self, key: TargetKey) -> PathBuf {
        self.skill_dir(key)
    }

    fn to_platform(&self, entry: &McpEntry, key: TargetKey) -> serde_json::Value {
        mcp::to_platform(entry, key)
    }

    fn from_platform(&self, raw: &serde_json::Value, key: TargetKey) -> Result<McpEntry> {
        mcp::from_platform(raw, key)
    }

    /// 契约 15：每次写都备份，不判断「值不值得」——丢配置的代价远大于这一行的成本。
    /// 原文件不存在时写一个 `null` 占位，恢复时表示「本来就没有，删掉即可」。
    fn backup_before_write(&self, key: TargetKey) -> Result<PathBuf> {
        let source = self.mcp_path(key);
        let dir = self
            .backups_dir()
            .join(Local::now().format("%Y%m%d-%H%M%S").to_string());
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::new("io", format!("建备份目录失败：{e}")))?;
        let dest = dir.join(format!("{}.json", targets::key_name(key)));
        if source.exists() {
            let bytes = std::fs::read(&source)
                .map_err(|e| AppError::new("io", format!("读配置失败：{e}")))?;
            std::fs::write(&dest, bytes)
                .map_err(|e| AppError::new("io", format!("写备份失败：{e}")))?;
        } else {
            std::fs::write(&dest, b"null")
                .map_err(|e| AppError::new("io", format!("写备份失败：{e}")))?;
        }
        Ok(dest)
    }

    /// 契约 16：同名条目存在且没有我们的标记 → 冲突，绝不覆盖。
    fn check_conflict(&self, name: &str, key: TargetKey) -> Result<()> {
        let root = mcp::read_config(&self.mcp_path(key));
        if let Some(body) = mcp::entries_of(&root, key).get(name) {
            if !self.owns_mcp(name, body) {
                return Err(AppError::new(
                    "conflict",
                    format!("{name} 已存在且不是本工具写入"),
                ));
            }
        }
        Ok(())
    }
}

/// 平台键的字符串形式（文件名与界面 key 共用）。
pub fn key_name(key: TargetKey) -> &'static str {
    targets::key_name(key)
}

/// 把一条 MCP 写进某个平台的配置文件：备份 → 冲突检测 → 读 → 改 → 写。
///
/// 顺序是有意的：**先备份再冲突检测**，冲突时用户手上也有一份刚才的配置。
pub fn write_entry(hub: &Hub, entry: &McpEntry, key: TargetKey) -> Result<PathBuf> {
    let backup = hub.backup_before_write(key)?;
    hub.check_conflict(&entry.name, key)?;

    let path = hub.mcp_path(key);
    let mut root = mcp::read_config(&path);
    let body = mcp::to_platform(entry, key);
    let body = mcp::entries_of(&body, key)
        .get(&entry.name)
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    if !root.is_object() {
        root = serde_json::json!({});
    }
    let container = mcp::container(key);
    insert(&mut root, container, &entry.name, body);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::new("io", format!("建目录失败：{e}")))?;
    }
    let text = serde_json::to_string_pretty(&root)
        .map_err(|e| AppError::new("io", format!("序列化失败：{e}")))?;
    crate::app::fsx::write_atomic(&path, text.as_bytes())?;
    Ok(backup)
}

fn insert(root: &mut serde_json::Value, container: &[&str], name: &str, body: serde_json::Value) {
    let mut node = root;
    for part in container {
        if !node.is_object() {
            *node = serde_json::json!({});
        }
        node = node
            .as_object_mut()
            .expect("上面刚保证是对象")
            .entry(part.to_string())
            .or_insert_with(|| serde_json::json!({}));
    }
    if !node.is_object() {
        *node = serde_json::json!({});
    }
    node.as_object_mut()
        .expect("上面刚保证是对象")
        .insert(name.to_string(), body);
}

/// 从配置文件里拿掉一条（只拿本工具写的，别人的不动）。
pub fn remove_entry(hub: &Hub, name: &str, key: TargetKey) -> Result<bool> {
    hub.check_conflict(name, key)?;
    let path = hub.mcp_path(key);
    if !path.exists() {
        return Ok(false);
    }
    hub.backup_before_write(key)?;
    let mut root = mcp::read_config(&path);
    let container = mcp::container(key);
    let mut node = &mut root;
    for part in container {
        match node.get_mut(*part) {
            Some(next) => node = next,
            None => return Ok(false),
        }
    }
    let removed = node
        .as_object_mut()
        .map(|map| map.remove(name).is_some())
        .unwrap_or(false);
    if removed {
        let text = serde_json::to_string_pretty(&root)
            .map_err(|e| AppError::new("io", format!("序列化失败：{e}")))?;
        crate::app::fsx::write_atomic(&path, text.as_bytes())?;
    }
    Ok(removed)
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
/// 界面需要的全部数据：技能 + MCP + 平台清单。一次读完，避免多次往返。
#[derive(Debug, Clone, Serialize)]
pub struct HubSnapshot {
    pub skills: Vec<SkillRow>,
    pub mcp: Vec<McpRow>,
    pub targets: Vec<TargetInfo>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct SkillRow {
    pub name: String,
    pub description: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub files: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: i64,
    pub valid: bool,
    pub reason: String,
    /// [(平台键, synced|stale|missing)]
    pub states: Vec<(String, &'static str)>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct McpRow {
    pub name: String,
    pub command: String,
    pub disabled: bool,
    /// 出现在哪些平台的配置里（只有本工具写的才算「部署到」）
    pub targets: Vec<String>,
    /// 该条目实际出现在哪些平台（含非本工具写入的），平台视图按这个过滤
    pub present: Vec<String>,
    /// 有同名条目但不是本工具写的：界面要标出来，避免用户以为可以随便删
    pub foreign: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct TargetInfo {
    pub key: String,
    pub label: String,
    pub skill_dir: String,
    pub mcp_file: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub skills: usize,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub mcp: usize,
    /// 平台技能目录里实际存在的技能（分发部署的 + 平台自带的）
    pub installed: Vec<skills::InstalledSkill>,
}

pub fn snapshot(hub: &Hub) -> HubSnapshot {
    let skills = skills::scan(hub)
        .into_iter()
        .map(|info| SkillRow {
            states: skills::states(hub, &info)
                .into_iter()
                .map(|(key, state)| (targets::key_name(key).to_string(), state))
                .collect(),
            name: info.name,
            description: info.description,
            files: info.files,
            size: info.size,
            valid: info.valid,
            reason: info.reason,
        })
        .collect();

    // MCP：按名字合并各平台，记下出现在哪、有没有别人的同名条目
    let mut merged: std::collections::BTreeMap<String, McpRow> = std::collections::BTreeMap::new();
    let mut target_infos = Vec::new();
    for target in targets::TARGETS {
        let key = targets::key_name(target.key);
        let root = mcp::read_config(&hub.mcp_path(target.key));
        let entries = mcp::entries_of(&root, target.key);
        let mut own = 0;
        for (name, body) in &entries {
            let ours = hub.owns_mcp(name, body);
            let entry = merged.entry(name.clone()).or_insert_with(|| {
                // 停用字段各平台的写法不同（ZCode 是 enabled 取反，其余是 disabled），
                // 必须经 from_body 按 style 解析——手拆会把正常条目全显示成「停用」
                let parsed = mcp::from_body(name, body, target.key);
                McpRow {
                    name: name.clone(),
                    command: parsed.command.unwrap_or_default(),
                    disabled: parsed.disabled,
                    targets: Vec::new(),
                    present: Vec::new(),
                    foreign: false,
                }
            });
            // 不管是谁写的，这条都出现在该平台里
            if !entry.present.iter().any(|t| t == key) {
                entry.present.push(key.to_string());
            }
            if ours {
                own += 1;
                if !entry.targets.iter().any(|t| t == key) {
                    entry.targets.push(key.to_string());
                }
            } else {
                entry.foreign = true;
            }
        }
        let skill_count = std::fs::read_dir(hub.skill_dir(target.key))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().is_dir() && e.path().join("SKILL.md").is_file())
                    .count()
            })
            .unwrap_or(0);
        target_infos.push(TargetInfo {
            key: key.to_string(),
            label: target.label.to_string(),
            skill_dir: hub.skill_dir(target.key).display().to_string(),
            mcp_file: hub.mcp_path(target.key).display().to_string(),
            skills: skill_count,
            mcp: own,
            installed: skills::installed(hub, target.key),
        });
    }

    HubSnapshot {
        skills,
        mcp: merged.into_values().collect(),
        targets: target_infos,
    }
}

/// 写一条 MCP 到多个平台。冲突的平台不覆盖，交给界面列出来。
pub fn mcp_add(hub: &Hub, entry: &McpEntry, keys: &[TargetKey]) -> DeployReport {
    let mut report = DeployReport::default();
    for key in keys {
        let name = targets::key_name(*key);
        match write_entry(hub, entry, *key) {
            Ok(_) => report.push(true, name, "已写入", &hub.mcp_path(*key), false),
            Err(e) if e.kind == crate::app::error::ErrorKind::Conflict => {
                report.push(false, name, e.message, &hub.mcp_path(*key), true)
            }
            Err(e) => report.push(false, name, e.message, &hub.mcp_path(*key), false),
        }
    }
    report
}

/// 把一条**已存在**的 MCP 条目写到更多平台（界面上每行的「部署」）。
/// 源条目从它所在的平台读回——优先本工具写入的（带标记、格式归一过），
/// 否则退回任意平台上的同名条目；写入走与粘贴导入相同的冲突检查。
pub fn mcp_deploy(hub: &Hub, name: &str, keys: &[TargetKey]) -> DeployReport {
    match mcp_source_entry(hub, name) {
        Some(entry) => mcp_add(hub, &entry, keys),
        None => {
            let mut report = DeployReport::default();
            report.push(
                false,
                name,
                "所有平台里都找不到这条 MCP，可能已被删除",
                &std::path::PathBuf::new(),
                false,
            );
            report
        }
    }
}

fn mcp_source_entry(hub: &Hub, name: &str) -> Option<McpEntry> {
    let mut fallback = None;
    for target in targets::TARGETS {
        let root = mcp::read_config(&hub.mcp_path(target.key));
        let entries = mcp::entries_of(&root, target.key);
        if let Some(body) = entries.get(name) {
            let entry = mcp::from_body(name, body, target.key);
            if hub.owns_mcp(name, body) {
                return Some(entry);
            }
            if fallback.is_none() {
                fallback = Some(entry);
            }
        }
    }
    fallback
}

/// 从多个平台删掉同一条（只删本工具写的）。
pub fn mcp_remove(hub: &Hub, name: &str, keys: &[TargetKey]) -> DeployReport {
    let mut report = DeployReport::default();
    for key in keys {
        let label = targets::key_name(*key);
        let path = hub.mcp_path(*key);
        match remove_entry(hub, name, *key) {
            Ok(true) => report.push(true, label, "已移除", &path, false),
            Ok(false) => report.push(true, label, "本来就没有", &path, false),
            Err(e) if e.kind == crate::app::error::ErrorKind::Conflict => {
                report.push(false, label, e.message, &path, true)
            }
            Err(e) => report.push(false, label, e.message, &path, false),
        }
    }
    report
}

/// 解析粘贴进来的 MCP 文本。认三种形状：标准 `mcpServers`、ZCode `mcp.servers`、
/// 单个条目（带 name / command），另外也认数组。
pub fn parse_text(text: &str) -> Result<Vec<McpEntry>> {
    let value: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|e| AppError::new("invalid", format!("JSON 解析失败：{e}")))?;
    let mut out = Vec::new();

    match &value {
        serde_json::Value::Array(items) => {
            for item in items {
                out.extend(entries_of_value(item)?);
            }
        }
        serde_json::Value::Object(map) => {
            if map.contains_key("mcpServers") {
                out.extend(entries_of_value(&value)?);
            } else if map.get("mcp").and_then(|v| v.get("servers")).is_some() {
                out.extend(from_map(
                    map.get("mcp")
                        .and_then(|v| v.get("servers"))
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default(),
                ));
            } else if map.contains_key("command") || map.contains_key("url") {
                let name = map
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AppError::new("invalid", "单个条目必须带 name"))?;
                out.push(mcp::from_body(name, &value, TargetKey::QoderCn));
            } else {
                // 也可能是 {名字: 条目} 的裸对象
                out.extend(from_map(map.clone()));
            }
        }
        _ => return Err(AppError::new("invalid", "这段文本不是 MCP 配置")),
    }
    if out.is_empty() {
        return Err(AppError::new("invalid", "没有解析出任何 MCP 条目"));
    }
    Ok(out)
}

fn entries_of_value(value: &serde_json::Value) -> Result<Vec<McpEntry>> {
    let map = value
        .as_object()
        .ok_or_else(|| AppError::new("invalid", "配置不是对象"))?;
    let entries = map.get("mcpServers").and_then(|v| v.as_object()).cloned();
    let zcode = map
        .get("mcp")
        .and_then(|v| v.get("servers"))
        .and_then(|v| v.as_object())
        .cloned();
    Ok(from_map(entries.or(zcode).unwrap_or_default()))
}

fn from_map(map: serde_json::Map<String, serde_json::Value>) -> Vec<McpEntry> {
    map.into_iter()
        .filter(|(_, body)| body.is_object())
        .map(|(name, body)| mcp::from_body(&name, &body, TargetKey::QoderCn))
        .collect()
}

pub fn read_entries(hub: &Hub, key: TargetKey) -> Vec<McpEntry> {
    let root = mcp::read_config(&hub.mcp_path(key));
    mcp::entries_of(&root, key)
        .into_iter()
        .map(|(name, body)| mcp::from_body(&name, &body, key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(dir: &std::path::Path) -> Hub {
        Hub::at(dir.join("home"), dir.join("appdata"), dir.join("data"))
    }

    fn entry(name: &str) -> McpEntry {
        McpEntry {
            name: name.into(),
            disabled: false,
            command: Some("npx".into()),
            ..Default::default()
        }
    }

    #[test]
    fn write_creates_config_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        let backup = write_entry(&hub, &entry("github"), TargetKey::QoderCn).unwrap();
        assert!(backup.exists());
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "null",
            "原来没有配置文件就写占位"
        );

        let path = hub.mcp_path(TargetKey::QoderCn);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"mcpServers\""));
        assert!(text.contains("github"));
    }

    #[test]
    fn foreign_entry_is_a_conflict_and_stays_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        let path = hub.mcp_path(TargetKey::Trae);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"{"mcpServers":{"github":{"command":"uvx"}}}"#;
        std::fs::write(&path, original).unwrap();

        let error = write_entry(&hub, &entry("github"), TargetKey::Trae).unwrap_err();
        assert_eq!(error.message, "github 已存在且不是本工具写入");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "冲突时原文件一个字节都不能变"
        );
    }

    #[test]
    fn we_can_overwrite_our_own_entry() {
        let dir = tempfile::tempdir().unwrap();
        let hub = hub(dir.path());
        write_entry(&hub, &entry("github"), TargetKey::QoderCn).unwrap();
        let mut changed = entry("github");
        changed.args = Some(vec!["-y".into()]);
        write_entry(&hub, &changed, TargetKey::QoderCn).unwrap();
        let raw = mcp::read_config(&hub.mcp_path(TargetKey::QoderCn));
        let parsed = mcp::from_platform(&raw, TargetKey::QoderCn).unwrap();
        assert_eq!(parsed.args, Some(vec!["-y".to_string()]));
    }
}
