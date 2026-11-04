// hub 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

// ------------------------------------------------------------ 技能与 MCP

pub fn target_keys(names: &[String]) -> Vec<crate::contracts::TargetKey> {
    names
        .iter()
        .filter_map(|name| crate::app::hub::targets::from_key_name(name))
        .collect()
}

/// 技能 + MCP + 平台清单，一次读完。
#[tauri::command]

pub async fn hub_snapshot(
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::hub::HubSnapshot, String> {
    let hub = state.hub.clone();
    crate::app::runtime::blocking(move || crate::app::hub::snapshot(&hub))
        .await
        .map_err(err_text)
}

/// 从某个平台批量把技能收进技能库（「从软件保存到库」）。
#[tauri::command]

pub async fn hub_skill_import_many(
    state: State<'_, AppState>,
    target: String,
    names: Vec<String>,
) -> std::result::Result<crate::app::hub::ImportReport, String> {
    let hub = state.hub.clone();
    let key = crate::app::hub::targets::from_key_name(&target).ok_or_else(|| {
        err_text(AppError::new(
            ErrorKind::Invalid,
            format!("未知平台：{target}"),
        ))
    })?;
    let report =
        crate::app::runtime::blocking(move || crate::app::hub::import_many(&hub, key, &names))
            .await
            .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?;
    log::log(
        if report.errors.is_empty() {
            Level::Info
        } else {
            Level::Warn
        },
        "hub",
        format!("从平台导入技能：{}", report.summary()),
    );
    Ok(report)
}

/// 部署技能到选中的平台。`overwrite=true` 才允许盖掉内容不同的同名副本。
#[tauri::command]

pub async fn hub_skill_deploy(
    state: State<'_, AppState>,
    name: String,
    targets: Vec<String>,
    overwrite: Option<bool>,
) -> std::result::Result<crate::app::hub::DeployReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    let overwrite = overwrite.unwrap_or(false);
    let report = crate::app::runtime::blocking(move || {
        crate::app::hub::skills::deploy(&hub, &name, &keys, overwrite)
    })
    .await
    .map_err(err_text)?
    .map_err(err_text)?;
    log::log(
        Level::Info,
        "hub",
        format!("部署技能：{}", report.summary()),
    );
    Ok(report)
}

/// 从平台移除技能（只删本工具部署的副本）。
#[tauri::command]

pub async fn hub_skill_remove(
    state: State<'_, AppState>,
    name: String,
    targets: Vec<String>,
) -> std::result::Result<crate::app::hub::DeployReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    let report =
        crate::app::runtime::blocking(move || crate::app::hub::skills::remove(&hub, &name, &keys))
            .await
            .map_err(err_text)?;
    log::log(
        Level::Info,
        "hub",
        format!("移除技能：{}", report.summary()),
    );
    Ok(report)
}

/// 从某个平台把技能导入库。
#[tauri::command]

pub async fn hub_skill_import(
    state: State<'_, AppState>,
    target: String,
    name: String,
) -> std::result::Result<crate::app::hub::skills::SkillInfo, String> {
    let hub = state.hub.clone();
    let key = crate::app::hub::targets::from_key_name(&target).ok_or_else(|| {
        err_text(AppError::new(
            ErrorKind::Invalid,
            format!("未知平台：{target}"),
        ))
    })?;
    crate::app::runtime::blocking(move || crate::app::hub::skills::import(&hub, key, &name))
        .await
        .map_err(err_text)?
        .map_err(err_text)
}

#[derive(serde::Serialize)]

pub struct McpImportReport {
    pub names: Vec<String>,
    pub report: crate::app::hub::DeployReport,
}

/// 粘贴一段 MCP 配置，写到选中的平台。
#[tauri::command]

pub async fn hub_mcp_import(
    state: State<'_, AppState>,
    text: String,
    targets: Vec<String>,
) -> std::result::Result<McpImportReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    if keys.is_empty() {
        return Err(err_text(AppError::new(
            ErrorKind::Invalid,
            "先选至少一个平台",
        )));
    }
    crate::app::runtime::blocking(move || {
        let entries = crate::app::hub::parse_text(&text)?;
        let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        let mut report = crate::app::hub::DeployReport::default();
        for entry in &entries {
            let one = crate::app::hub::mcp_add(&hub, entry, &keys);
            report.steps.extend(one.steps);
        }
        Ok(McpImportReport { names, report })
    })
    .await
    .map_err(err_text)?
    .map_err(err_text)
}

/// 从选中的平台删掉一条 MCP（只删本工具写的）。
#[tauri::command]

pub async fn hub_mcp_remove(
    state: State<'_, AppState>,
    name: String,
    targets: Vec<String>,
) -> std::result::Result<crate::app::hub::DeployReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    crate::app::runtime::blocking(move || crate::app::hub::mcp_remove(&hub, &name, &keys))
        .await
        .map_err(err_text)
}

/// 把一条已存在的 MCP 条目写到更多平台（MCP 表每行的「部署」）。
#[tauri::command]

pub async fn hub_mcp_deploy(
    state: State<'_, AppState>,
    name: String,
    targets: Vec<String>,
) -> std::result::Result<crate::app::hub::DeployReport, String> {
    let hub = state.hub.clone();
    let keys = target_keys(&targets);
    crate::app::runtime::blocking(move || crate::app::hub::mcp_deploy(&hub, &name, &keys))
        .await
        .map_err(err_text)
}
