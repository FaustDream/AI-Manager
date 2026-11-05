// maintenance 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

/// 数据目录占用。
#[tauri::command]

pub async fn maintenance_space() -> crate::app::maintenance::SpaceReport {
    // 递归遍历整个 data/（含会话正文），主线程上跑会卡住界面
    crate::app::runtime::blocking(crate::app::maintenance::space)
        .await
        .unwrap_or_default()
}

/// 回收站批次（会话删除与平台技能移除都算）。
#[tauri::command]

pub async fn maintenance_trash() -> Vec<crate::app::maintenance::SpaceEntry> {
    crate::app::runtime::blocking(crate::app::maintenance::trash_batches)
        .await
        .unwrap_or_default()
}

/// 清空回收站：`tag=None` 全清。返回释放的字节数。
#[tauri::command]

pub async fn maintenance_purge_trash(tag: Option<String>) -> std::result::Result<i64, String> {
    crate::app::runtime::blocking(move || crate::app::maintenance::purge_trash(tag.as_deref()))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 打开数据目录。
#[tauri::command]

pub fn maintenance_open_data() -> std::result::Result<(), String> {
    crate::app::maintenance::open_data_dir().map_err(err_text)
}
