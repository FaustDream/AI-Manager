//! 账号维度的会话恢复：**只改归属引用，一个字节的正文都不动**（契约 10）。
//!
//! 场景：换号之后旧账号的会话还在库里，但当前账号的列表看不到（`user_id` 不匹配）。
//! 「恢复」= 把 `sessions.user_id` 改成当前账号，正文、任务、待办原地不动。
//!
//! 写客户端库前必须 checkpoint，写前留一份库文件备份——这一步不可逆，
//! 备份是唯一能退回去的路。

use crate::app::clients::Roots;
use crate::app::error::{AppError, Result};
use crate::app::fsx;
use crate::app::paths;
use crate::app::sessions::sources::wb;
use crate::contracts::RestoreReport;
use chrono::Local;
use rusqlite::OpenFlags;

/// `edition` 取 `cn` / `intl`。`uid` 是目标账号的 uid。
pub fn account(roots: &Roots, edition: &str, uid: &str) -> Result<RestoreReport> {
    if edition != "cn" && edition != "intl" {
        return Err(AppError::new("invalid", format!("未知版本：{edition}")));
    }
    if uid.trim().is_empty() {
        return Err(AppError::new("invalid", "目标账号为空"));
    }
    let ep = wb::editions(roots)
        .into_iter()
        .find(|item| item.key == edition)
        .ok_or_else(|| AppError::new("invalid", format!("{edition} 的会话库不存在")))?;

    let files_before = wb::project_files(&ep.projects)
        .values()
        .map(|v| v.len())
        .sum::<usize>();

    // 契约：不在客户端运行时写它的 SQLite。走与删除同一道进程闸，
    // 关不掉就拒绝执行——写到一半被客户端抢回去比不做更糟
    let closed = crate::app::sessions::delete::close_clients(&["workbuddy"]).0;
    if crate::app::process::process_running(crate::app::accounts::add::process_names("workbuddy")) {
        return Err(AppError::new(
            "client_running",
            "WorkBuddy 正在运行且无法自动关闭，请从托盘退出后再恢复会话",
        ));
    }

    // 写前备份：整份库文件复制出去，回滚就是把它盖回来
    let tag = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let backup = fsx::backup_to(
        &ep.db(),
        &paths::data_dir().join("backups").join("session-restore"),
        &tag,
    )?;

    let conn = rusqlite::Connection::open_with_flags(ep.db(), OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| AppError::new("sqlite", format!("打开 {} 失败：{e}", ep.db().display())))?;
    crate::app::sessions::delete::checkpoint(&conn)?;

    let changed = conn
        .execute(
            "UPDATE sessions SET user_id = ?1
             WHERE transport = 'local' AND user_id <> ?1",
            [uid],
        )
        .map_err(|e| AppError::new("sqlite", format!("改归属失败：{e}")))?;

    crate::app::sessions::delete::checkpoint(&conn)?;

    let files_after = wb::project_files(&ep.projects)
        .values()
        .map(|v| v.len())
        .sum::<usize>();
    // 客户端是为这次写入关掉的，走完要把窗口还回去
    if !closed.is_empty() {
        let _ = crate::app::sessions::delete::restart(&closed);
    }
    Ok(RestoreReport {
        changed_rows: changed,
        body_files_before: files_before,
        body_files_after: files_after,
        backup,
    })
}
