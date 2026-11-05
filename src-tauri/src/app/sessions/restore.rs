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
use crate::app::log;
use crate::app::sessions::sources::wb;
use crate::contracts::RestoreReport;
use rusqlite::OpenFlags;

/// 界面「恢复会话」的结果：比契约报告少几个内部字段，只留用户看得懂的。
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoreOutcome {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub changed_rows: usize,
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
    let edition = crate::app::accounts::edition_of(account_id);
    // 主体名：库里查得到就用昵称，查不到退回 uid
    let subject = crate::app::accounts::repo::identity(db, iid)
        .ok()
        .flatten()
        .map(|identity| identity.name)
        .unwrap_or_else(|| iid.to_string());
    let report = account(
        roots,
        edition,
        crate::app::accounts::uid_of(account_id),
        &subject,
    )?;
    Ok(RestoreOutcome {
        changed_rows: report.changed_rows,
        edition: edition.to_string(),
    })
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

/// `edition` 取 `cn` / `intl`。`uid` 是目标账号的 uid，`subject` 是执行记录里的主体名。
pub fn account(roots: &Roots, edition: &str, uid: &str, subject: &str) -> Result<RestoreReport> {
    run(roots, edition, uid, None, subject)
}

/// 账号切换专用：只搬「切换前那个账号」名下的会话，别把同一版本里
/// 其他账号的会话也划过来（手动恢复才允许全量）。
pub fn from_account(
    roots: &Roots,
    edition: &str,
    uid: &str,
    source: &str,
    subject: &str,
) -> Result<RestoreReport> {
    run(roots, edition, uid, Some(source), subject)
}

fn run(
    roots: &Roots,
    edition: &str,
    uid: &str,
    from: Option<&str>,
    subject: &str,
) -> Result<RestoreReport> {
    let started = std::time::Instant::now();
    // 失败一律走这里：环节 + 根因 + 处置，缺一个用户就得自己猜
    let fail = |step: &str, cause: &str, action: &str, e: &AppError| {
        log::item(
            log::Item::new("恢复会话", subject, log::Outcome::Fail)
                .step(step)
                .reason(e.message.clone())
                .evidence(format!("根因={cause} | 处置={action} | 版本={edition}"))
                .ms(started.elapsed().as_millis() as u64),
        );
    };

    log::item(log::Item::marker("恢复会话", subject).note(format!(
        "开始 | 版本={edition} | 来源={}",
        from.unwrap_or("全量对齐")
    )));

    if edition != "cn" && edition != "intl" {
        let e = AppError::new(ErrorKind::Invalid, format!("未知版本：{edition}"));
        fail("校验版本", "版本参数不合法", "无需处置", &e);
        return Err(e);
    }
    if uid.trim().is_empty() {
        let e = AppError::new(ErrorKind::Invalid, "目标账号为空");
        fail("校验账号", "目标账号为空", "重新选择账号", &e);
        return Err(e);
    }
    let ep = match wb::editions(roots)
        .into_iter()
        .find(|item| item.key == edition)
    {
        Some(ep) => ep,
        None => {
            let e = AppError::new(ErrorKind::Invalid, format!("{edition} 的会话库不存在"));
            fail(
                "查找会话库",
                "该版本没有会话库（客户端未安装或从未登录）",
                "先在客户端登录一次",
                &e,
            );
            return Err(e);
        }
    };

    let files_before = wb::project_files(&ep.projects)
        .values()
        .map(|v| v.len())
        .sum::<usize>();

    // 契约：不在客户端运行时写它的 SQLite。关不掉就拒绝执行——写到一半被客户端
    // 抢回去比不做更糟。**按版本关**：写的是哪个版本的库就只关哪个版本的客户端，
    // 而且只关它自己的 exe（国内/国际是两个进程，CodeBuddy IDE 是另一个产品）。
    let key = client_key(edition);
    let closed = match crate::app::accounts::add::close_client(key) {
        Ok(closed) => closed,
        Err(e) => {
            fail(
                "关闭客户端",
                "客户端未能退出（可能在托盘）",
                "从托盘退出后重试",
                &e,
            );
            return Err(e);
        }
    };
    log::item(
        log::Item::new("恢复会话", subject, log::Outcome::Ok)
            .step("关闭客户端")
            .reason(if closed {
                "客户端已退出"
            } else {
                "客户端本来就没开"
            })
            .evidence(format!("进程={key}"))
            .ms(started.elapsed().as_millis() as u64),
    );

    let conn =
        match rusqlite::Connection::open_with_flags(ep.db(), OpenFlags::SQLITE_OPEN_READ_WRITE) {
            Ok(conn) => conn,
            Err(e) => {
                let err = AppError::new(
                    ErrorKind::Sqlite,
                    format!("打开 {} 失败：{e}", ep.db().display()),
                );
                fail(
                    "打开会话库",
                    "会话库被占用或已损坏",
                    "关闭客户端后重试",
                    &err,
                );
                return Err(err);
            }
        };
    if let Err(e) = crate::app::sessions::delete::checkpoint(&conn) {
        fail(
            "落盘预检",
            "checkpoint 失败（库被占用）",
            "关闭客户端后重试",
            &e,
        );
        return Err(e);
    }

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
    .map_err(|e| AppError::new(ErrorKind::Sqlite, format!("改归属失败：{e}")));

    let changed = match changed {
        Ok(changed) => changed,
        Err(e) => {
            fail("改写归属", "写会话库失败", "重新登录该账号后可重试", &e);
            return Err(e);
        }
    };

    if let Err(e) = crate::app::sessions::delete::checkpoint(&conn) {
        fail("落盘", "checkpoint 失败", "重试一次", &e);
        return Err(e);
    }

    let files_after = wb::project_files(&ep.projects)
        .values()
        .map(|v| v.len())
        .sum::<usize>();
    // 客户端是为这次写入关掉的，走完要把窗口还回去（还是关掉的那一版）
    if closed {
        let _ = crate::app::sessions::delete::restart(&[key.to_string()]);
    }

    // 收尾交代「改了多少、正文有没有被动过、下一步要不要人工」：
    // 正文文件数不变才说明只是改了归属引用，没碰内容
    log::item(
        log::Item::new(
            "恢复会话",
            subject,
            if files_before == files_after {
                log::Outcome::Ok
            } else {
                log::Outcome::Partial
            },
        )
        .step("改写归属")
        .reason(format!("关联 {changed} 条会话"))
        .evidence(format!(
            "正文文件 {}→{} | 版本={edition} | 客户端={}",
            files_before,
            files_after,
            if closed { "已重新打开" } else { "未动" }
        ))
        .ms(started.elapsed().as_millis() as u64),
    );

    Ok(RestoreReport {
        changed_rows: changed,
        body_files_before: files_before,
        body_files_after: files_after,
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
        use crate::app::accounts::{edition_of, uid_of};
        assert_eq!(edition_of("u1@intl"), "intl");
        assert_eq!(edition_of("u1"), "cn");
        assert_eq!(uid_of("u1@intl"), "u1");
    }
}
