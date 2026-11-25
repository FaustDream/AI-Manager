// maintenance 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

/// 数据目录占用。
pub async fn maintenance_space(
) -> std::result::Result<crate::app::maintenance::SpaceReport, String> {
    // 递归遍历整个 data/（含会话正文），主线程上跑会卡住界面。
    // 遍历本身不会失败，但后台任务崩溃（blocking 的 join 错误）必须如实报出：
    // 早先的 unwrap_or_default() 会把它压成「0 占用」的假空报告，用户看不出出错。
    crate::app::runtime::blocking(crate::app::maintenance::space)
        .await
        .map_err(err_text)
}

/// 回收站批次（平台技能移除时落下的副本）。
pub async fn maintenance_trash(
) -> std::result::Result<Vec<crate::app::maintenance::SpaceEntry>, String> {
    crate::app::runtime::blocking(crate::app::maintenance::trash_batches)
        .await
        .map_err(err_text)
}

/// 清空回收站：`tag=None` 全清。返回释放的字节数。
pub async fn maintenance_purge_trash(tag: Option<String>) -> std::result::Result<i64, String> {
    crate::app::runtime::blocking(move || crate::app::maintenance::purge_trash(tag.as_deref()))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 打开数据目录。
pub fn maintenance_open_data() -> std::result::Result<(), String> {
    crate::app::maintenance::open_data_dir().map_err(err_text)
}
