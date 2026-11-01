//! MCP 配置的统一表示 ↔ 各平台文件格式（契约 14）。
//!
//! 内部一律用 `disabled` 表示停用；ZCode 那边是 `enabled: false`，写的时候取反。
//! 每个平台保留自己那份配置，**不做全局合并**（`docs/phase-5-hub.md` §5.1）——
//! 只有在「部署到新平台」时才用统一格式生成一份默认值。
//!
//! 本工具写进去的条目带一个 `_aimanager: true` 标记：冲突检测靠它区分
//! 「这是我们在管的」和「这是用户自己加的」。标记跟着配置走，用户把配置拷到
//! 另一台机器也认得出，比在本地库里记一份 `deployed` 更可靠。

use crate::app::error::{AppError, Result};
use crate::app::hub::targets::{self, McpStyle};
use crate::contracts::{McpEntry, TargetKey};
use serde_json::{json, Map, Value};

pub const MARKER: &str = "_aimanager";

/// 容器路径：标准 `mcpServers`，ZCode 是 `mcp.servers`。
pub fn container(key: TargetKey) -> &'static [&'static str] {
    match targets::get(key).style {
        McpStyle::ZCode => &["mcp", "servers"],
        _ => &["mcpServers"],
    }
}

fn entry_json(entry: &McpEntry, key: TargetKey) -> Value {
    let style = targets::get(key).style;
    let mut map = Map::new();
    map.insert(MARKER.into(), Value::Bool(true));
    if let Some(command) = &entry.command {
        map.insert("command".into(), Value::String(command.clone()));
    }
    if let Some(args) = &entry.args {
        map.insert(
            "args".into(),
            Value::Array(args.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(env) = &entry.env {
        let mut object = Map::new();
        for (name, value) in env {
            object.insert(name.clone(), Value::String(value.clone()));
        }
        map.insert("env".into(), Value::Object(object));
    }

    match style {
        McpStyle::ZCode => {
            map.insert(
                "type".into(),
                Value::String(
                    if entry.command.is_some() {
                        "stdio"
                    } else {
                        "http"
                    }
                    .into(),
                ),
            );
            // 语义相反：我们存 disabled，ZCode 认 enabled
            map.insert("enabled".into(), Value::Bool(!entry.disabled));
            if let Some(timeout) = entry.timeout_ms {
                map.insert("timeoutMs".into(), Value::from(timeout));
            }
        }
        McpStyle::Trae => {
            map.insert("disabled".into(), Value::Bool(entry.disabled));
            // Trae 的 stdio 条目习惯带这两个空值，缺了它自己的界面会显示「未配置」
            if entry.command.is_some() {
                map.insert("url".into(), Value::String(String::new()));
                map.insert("headers".into(), Value::Object(Map::new()));
            }
        }
        McpStyle::CodeBuddy | McpStyle::Standard => {
            map.insert("disabled".into(), Value::Bool(entry.disabled));
        }
    }
    if let Some(timeout) = entry.timeout_ms {
        map.entry("timeoutMs").or_insert(Value::from(timeout));
    }
    Value::Object(map)
}

/// 一条条目 → 一份完整的平台配置文件（形如 `{"mcpServers": {...}}`）。
pub fn to_platform(entry: &McpEntry, key: TargetKey) -> Value {
    let mut entry_map = Map::new();
    entry_map.insert(entry.name.clone(), entry_json(entry, key));
    let mut root = Map::new();
    insert_container(&mut root, container(key), Value::Object(entry_map));
    Value::Object(root)
}

fn insert_container(root: &mut Map<String, Value>, path: &[&str], value: Value) {
    if path.len() == 1 {
        root.insert(path[0].to_string(), value);
        return;
    }
    let slot = root
        .entry(path[0].to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !slot.is_object() {
        *slot = Value::Object(Map::new());
    }
    if let Some(map) = slot.as_object_mut() {
        insert_container(map, &path[1..], value);
    }
}

/// 从平台配置文件里取出条目表（不存在就是空）。
pub fn entries_of(root: &Value, key: TargetKey) -> Map<String, Value> {
    let mut node = root;
    for part in container(key) {
        match node.get(*part) {
            Some(next) => node = next,
            None => return Map::new(),
        }
    }
    node.as_object().cloned().unwrap_or_default()
}

/// 一份平台配置 → 统一表示。取第一条；空配置报错。
pub fn from_platform(raw: &Value, key: TargetKey) -> Result<McpEntry> {
    let entries = entries_of(raw, key);
    let (name, body) = entries
        .iter()
        .next()
        .ok_or_else(|| AppError::new("invalid", "配置里没有条目"))?;
    Ok(from_body(name, body, key))
}

/// 单条：平台条目体 → 统一表示。名字以容器里的键为准（有的平台不写 name 字段）。
pub fn from_body(name: &str, body: &Value, key: TargetKey) -> McpEntry {
    let object = body.as_object().cloned().unwrap_or_default();

    let disabled = match targets::get(key).style {
        McpStyle::ZCode => !object
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        _ => object
            .get("disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };
    let command = object
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let args = object.get("args").and_then(|v| v.as_array()).map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect()
    });
    let env = object.get("env").and_then(|v| v.as_object()).map(|map| {
        map.iter()
            .filter_map(|(k, v)| v.as_str().map(|text| (k.clone(), text.to_string())))
            .collect()
    });
    let timeout_ms = object.get("timeoutMs").and_then(|v| v.as_u64());

    McpEntry {
        name: object
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| name.to_string()),
        disabled,
        command,
        args,
        env,
        timeout_ms,
    }
}

/// 这个条目是不是本工具写进去的（没有标记就不是，冲突检测靠它）。
pub fn written_by_us(body: &Value) -> bool {
    body.get(MARKER).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 读配置文件；不存在或坏掉都当空配置（写的时候会整体覆盖，坏文件先备份）。
pub fn read_config(path: &std::path::Path) -> Value {
    let Ok(text) = std::fs::read_to_string(path) else {
        return json!({});
    };
    serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> McpEntry {
        McpEntry {
            name: "github".into(),
            disabled: true,
            command: Some("npx".into()),
            args: Some(vec![
                "-y".into(),
                "@modelcontextprotocol/server-github".into(),
            ]),
            env: Some(vec![("TOKEN".into(), "x".into())]),
            timeout_ms: Some(30_000),
        }
    }

    #[test]
    fn roundtrip_keeps_semantics_for_every_style() {
        for key in [
            TargetKey::QoderCn,
            TargetKey::Trae,
            TargetKey::CodeBuddy,
            TargetKey::ZCode,
            TargetKey::WorkBuddy,
        ] {
            let raw = to_platform(&sample(), key);
            let back = from_platform(&raw, key).unwrap();
            assert!(back.disabled, "{key:?} 停用状态反了");
            assert_eq!(back.name, "github");
            assert_eq!(back.command.as_deref(), Some("npx"));
            assert_eq!(back.args.as_ref().map(|a| a.len()), Some(2));
        }
    }

    #[test]
    fn zcode_uses_enabled_and_type() {
        let raw = to_platform(&sample(), TargetKey::ZCode);
        let entry = raw
            .get("mcp")
            .and_then(|v| v.get("servers"))
            .and_then(|v| v.get("github"))
            .unwrap();
        assert_eq!(
            entry.get("enabled"),
            Some(&Value::Bool(false)),
            "disabled 要翻成 enabled:false"
        );
        assert_eq!(entry.get("type").and_then(|v| v.as_str()), Some("stdio"));
        assert_eq!(
            entry.get("timeoutMs").and_then(|v| v.as_u64()),
            Some(30_000)
        );
    }

    #[test]
    fn trae_fills_empty_http_fields() {
        let raw = to_platform(&sample(), TargetKey::Trae);
        let entry = raw.get("mcpServers").and_then(|v| v.get("github")).unwrap();
        assert_eq!(entry.get("url"), Some(&Value::String(String::new())));
        assert!(entry.get("headers").map(|v| v.is_object()).unwrap_or(false));
    }

    #[test]
    fn marker_separates_ours_from_users() {
        let ours = to_platform(&sample(), TargetKey::QoderCn);
        let mine = ours
            .get("mcpServers")
            .and_then(|v| v.get("github"))
            .unwrap();
        assert!(written_by_us(mine));
        let theirs = json!({"mcpServers": {"github": {"command": "npx"}}});
        let body = theirs
            .get("mcpServers")
            .and_then(|v| v.get("github"))
            .unwrap();
        assert!(!written_by_us(body));
    }

    #[test]
    fn empty_config_is_an_error_not_a_panic() {
        assert!(from_platform(&json!({}), TargetKey::Trae).is_err());
        assert!(from_platform(&json!({"mcpServers": {}}), TargetKey::Trae).is_err());
    }
}
