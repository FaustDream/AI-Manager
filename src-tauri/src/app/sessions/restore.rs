//! 账号维度的会话恢复：**只改归属引用，一个字节的正文都不动**（契约 10）。
//!
//! 场景：换号之后旧账号的会话还在库里，但当前账号的列表看不到（`user_id` 不匹配）。
//! 「恢复」= 把 `sessions.user_id` 改成当前账号，正文、任务、待办原地不动。
//!
//! 写客户端库前必须 checkpoint，写前留一份库文件备份——这一步不可逆，
//! 备份是唯一能退回去的路。

use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::fsx;
use crate::app::paths;
use crate::app::sessions::sources::wb;
use crate::contracts::RestoreReport;
use chrono::Local;
use rusqlite::OpenFlags;

/// 界面「恢复会话」的结果：比契约报告少几个内部字段，只留用户看得懂的。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoreOutcome {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub changed_rows: usize,
    /// 写客户端库之前的整库备份，界面只用来提示「后悔了去哪找」
    pub backup: Option<String>,
    /// `cn` / `intl`
    pub edition: String,
}

/// 界面入口：只允许对**当前登录**的 WorkBuddy 账号恢复。
/// 恢复的是「本版本里已有的全部会话」，等价 v1 手动点「恢复会话」的口径。
pub fn for_account(db: &Db, roots: &Roots, iid: &str) -> Result<RestoreOutcome> {
    let (platform, account_id) = iid
        .split_once(':')
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "账号标识不合法"))?;
    if platform != "workbuddy" {
        return Err(AppError::new(
            ErrorKind::Unsupported,
            "只有 WorkBuddy 有本机会话库；Qoder / ZCode / Trae 不涉及会话恢复",
        ));
    }
    if !crate::app::accounts::switch::is_current(db, iid)? {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "只能恢复当前登录账号的会话：先切换到该账号，再点恢复",
        ));
    }
    let edition = edition_of(account_id);
    let report = account(roots, edition, account_id.trim_end_matches("@intl"))?;
    Ok(RestoreOutcome {
        changed_rows: report.changed_rows,
        backup: Some(report.backup.display().to_string()),
        edition: edition.to_string(),
    })
}

/// WorkBuddy 两版共用 `workbuddy` 平台键，靠 `@intl` 后缀区分版本。
fn edition_of(account_id: &str) -> &'static str {
    if account_id.ends_with("@intl") {
        "intl"
    } else {
        "cn"
    }
}

/// 版本 → 客户端平台键（`accounts::add` 的进程名/安装路径用的是这套键）。
/// 国内/国际是两个独立进程，这里必须分开，否则会关掉/拉起另一个版本。
fn client_key(edition: &str) -> &'static str {
    if edition == "intl" {
        "workbuddy_intl"
    } else {
        "workbuddy"
    }
}

/// `edition` 取 `cn` / `intl`。`uid` 是目标账号的 uid。
pub fn account(roots: &Roots, edition: &str, uid: &str) -> Result<RestoreReport> {
    run(roots, edition, uid, None)
}

/// 账号切换专用：只搬「切换前那个账号」名下的会话，别把同一版本里
/// 其他账号的会话也划过来（手动恢复才允许全量）。
pub fn from_account(
    roots: &Roots,
    edition: &str,
    uid: &str,
    source: &str,
) -> Result<RestoreReport> {
    run(roots, edition, uid, Some(source))
}

fn run(roots: &Roots, edition: &str, uid: &str, from: Option<&str>) -> Result<RestoreReport> {
    if edition != "cn" && edition != "intl" {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("未知版本：{edition}"),
        ));
    }
    if uid.trim().is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "目标账号为空"));
    }
    let ep = wb::editions(roots)
        .into_iter()
        .find(|item| item.key == edition)
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, format!("{edition} 的会话库不存在")))?;

    let files_before = wb::project_files(&ep.projects)
        .values()
        .map(|v| v.len())
        .sum::<usize>();

    // 契约：不在客户端运行时写它的 SQLite。关不掉就拒绝执行——写到一半被客户端
    // 抢回去比不做更糟。**按版本关**：写的是哪个版本的库就只关哪个版本的客户端，
    // 而且只关它自己的 exe（国内/国际是两个进程，CodeBuddy IDE 是另一个产品）。
    let key = client_key(edition);
    let closed = crate::app::accounts::add::close_client(key)?;

    // 写前备份：整份库文件复制出去，回滚就是把它盖回来
    let tag = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let backup = fsx::backup_to(
        &ep.db(),
        &paths::data_dir().join("backups").join("session-restore"),
        &tag,
    )?;

    let conn = rusqlite::Connection::open_with_flags(ep.db(), OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| {
            AppError::new(
                ErrorKind::Sqlite,
                format!("打开 {} 失败：{e}", ep.db().display()),
            )
        })?;
    crate::app::sessions::delete::checkpoint(&conn)?;

    let changed = match from {
        // 来源就是目标：没有可搬的，别退化成「全量对齐」把别人的会话也划过来
        Some(source) if source == uid => Ok(0usize),
        Some(source) => conn.execute(
            "UPDATE sessions SET user_id = ?1
             WHERE transport = 'local' AND user_id = ?2",
            [uid, source],
        ),
        // 没有来源账号（手动恢复）：全量对齐
        None => conn.execute(
            "UPDATE sessions SET user_id = ?1
             WHERE transport = 'local' AND user_id <> ?1",
            [uid],
        ),
    }
    .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("改归属失败：{e}")))?;

    crate::app::sessions::delete::checkpoint(&conn)?;

    let files_after = wb::project_files(&ep.projects)
        .values()
        .map(|v| v.len())
        .sum::<usize>();
    // 客户端是为这次写入关掉的，走完要把窗口还回去（还是关掉的那一版）
    if closed {
        let _ = crate::app::sessions::delete::restart(&[key.to_string()]);
    }
    Ok(RestoreReport {
        changed_rows: changed,
        body_files_before: files_before,
        body_files_after: files_after,
        backup,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 恢复只对 WorkBuddy 有意义；别的平台在碰任何文件之前就要被挡住。
    #[test]
    fn other_platforms_are_rejected_before_touching_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let roots = Roots::at(dir.path(), dir.path(), dir.path());
        let err = for_account(&db, &roots, "qoder:u1").unwrap_err();
        assert_eq!(err.kind, crate::app::error::ErrorKind::Unsupported);
    }

    /// 国际版靠 `@intl` 后缀选版本，uid 要还原成库里的原值。
    #[test]
    fn intl_suffix_selects_intl_edition() {
        assert_eq!(edition_of("u1@intl"), "intl");
        assert_eq!(edition_of("u1"), "cn");
        assert_eq!("u1@intl".trim_end_matches("@intl"), "u1");
    }
}
