//! 切换登录的编排层：取账号 → 解凭据 → 交给平台实现 → 汇报结果。
//!
//! **这一层不碰客户端文件**。写入面是各家自己的事（Qoder 重封信封、WorkBuddy 换档案、
//! ZCode 整份 JSON），编排层只保证两件事：
//!   1. 读凭据**只读一个账号**（`repo::secret_of`），不批量解；
//!   2. 写客户端之前先把它关掉——写一半被客户端抢回去比不做更糟。

use super::credential::Credential;
use super::repo;
use super::{edition_of, uid_of};
use crate::app::clients::Roots;
use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::log;
use crate::app::{process, providers};
use serde::Serialize;

#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, export_to = "../../src/lib/types/"))]
#[derive(Debug, Clone, Serialize)]
pub struct SwitchResult {
    pub message: String,
}

/// 切换登录。会写客户端文件，失败代价是「客户端跳登录页」。
pub fn switch(db: &Db, roots: &Roots, iid: &str) -> Result<SwitchResult> {
    let started = std::time::Instant::now();
    let identity = repo::identity(db, iid)?
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "账号不存在，可能已被删除"))?;
    let subject = identity.name.clone();
    let platform = identity.platform_label.clone();
    log::item(
        log::Item::marker("切换账号", &subject)
            .note(format!("开始 | 目标={subject} | 平台={platform}")),
    );

    // 任何一步失败都要交代「现在停在哪」：切号最怕的不是失败，是账号串了没人知道
    let fail = |step: &str, e: &AppError, extra: &str| {
        log::item(
            log::Item::new("切换账号", &subject, log::Outcome::Fail)
                .step(step)
                .reason(e.message.clone())
                .evidence(format!("平台={platform} {extra}"))
                .ms(started.elapsed().as_millis() as u64),
        );
    };

    let secret = match repo::secret_of(db, iid)? {
        Some(secret) => secret,
        None => {
            let e = AppError::new(ErrorKind::Invalid, "这个账号没有保存登录凭据，无法切换");
            fail("读取凭据", &e, "凭据=未保存");
            return Err(e);
        }
    };
    let cred = match Credential::parse(&secret) {
        Ok(cred) => cred,
        Err(e) => {
            fail("读取凭据", &e, "凭据=解析失败");
            return Err(e);
        }
    };
    log::item(
        log::Item::new("切换账号", &subject, log::Outcome::Ok)
            .step("读取凭据")
            .reason("凭据可用")
            .evidence(format!("平台={platform} 来源=本机加密库"))
            .ms(started.elapsed().as_millis() as u64),
    );

    // 切换前先记下客户端当前登录的账号：切换后它就是「这些会话的旧主人」。
    // 必须写在 providers::switch 之前，之后读到的已经是新账号了。
    let previous = if providers::platform_of(iid) == Some("workbuddy") {
        providers::for_iid(iid).and_then(|provider| provider.current_id())
    } else {
        None
    };

    // 写客户端文件必须在进程退出之后，所以这里按**版本**先把对应客户端关掉：
    // WorkBuddy 国内/国际是两个进程，`client_key_of` 带着 `@intl` 一起判断，
    // 免得切国际版时去关国内版（或反过来）。关不掉就直接报错，一个字节都不写。
    let key = providers::client_key_of(iid);
    let closed = match super::add::close_client(key) {
        Ok(closed) => closed,
        Err(e) => {
            fail("关闭客户端", &e, "处置=未写入任何文件");
            return Err(e);
        }
    };
    log::item(
        log::Item::new("切换账号", &subject, log::Outcome::Ok)
            .step("关闭客户端")
            .reason(if closed {
                "客户端已退出"
            } else {
                "客户端本来就没开"
            })
            .evidence(format!("进程={key}"))
            .ms(started.elapsed().as_millis() as u64),
    );

    let outcome = match providers::switch(iid, &identity.account_id, &identity.name, &cred) {
        Ok(outcome) => outcome,
        Err(e) => {
            // 切换失败也要把客户端还回去，别让用户面对一个被关掉的软件
            if closed {
                start_client(key);
            }
            fail(
                "写入登录态",
                &e,
                &format!(
                    "处置=客户端已{}",
                    if closed {
                        "重新打开"
                    } else {
                        "保持原状"
                    }
                ),
            );
            return Err(e);
        }
    };
    log::item(
        log::Item::new("切换账号", &subject, log::Outcome::Ok)
            .step("写入登录态")
            .reason("客户端登录态已改写并验读通过")
            .evidence(format!("平台={platform}"))
            .ms(started.elapsed().as_millis() as u64),
    );

    let mut message = outcome.message;

    // WorkBuddy：切完把旧账号名下的会话按引用关联到新账号（契约 10）。
    // 必须在客户端重新启动之前做——写客户端库要求进程已退出。
    // clone：收尾那行还要用它写「原账号是谁」
    if let Some(previous) = previous.clone() {
        let uid = uid_of(&identity.account_id);
        let edition = edition_of(&identity.account_id);
        let source = uid_of(&previous);
        match crate::app::sessions::restore::from_account(roots, edition, uid, source, &subject) {
            Ok(report) if report.changed_rows > 0 => {
                message.push_str(&format!("，已关联 {} 条历史会话", report.changed_rows));
                log::item(
                    log::Item::new("切换账号", &subject, log::Outcome::Ok)
                        .step("关联历史会话")
                        .reason(format!("已关联 {} 条", report.changed_rows))
                        .evidence(format!("版本={edition}"))
                        .ms(started.elapsed().as_millis() as u64),
                );
            }
            Ok(_) => {}
            Err(e) => {
                // 会话没关联上不影响登录态已经切好：给用户一条能自己重试的路
                log::item(
                    log::Item::new("切换账号", &subject, log::Outcome::Partial)
                        .step("关联历史会话")
                        .reason(e.message.clone())
                        .evidence("处置=登录态已切好，可在账号上点「恢复会话」重试")
                        .ms(started.elapsed().as_millis() as u64),
                );
                message.push_str("，历史会话未关联（可在账号上点「恢复会话」重试）");
            }
        }
    }

    // 切完把客户端以新账号打开：**exe 也要按版本取**，只拿平台前缀会开成国内版。
    // 启动失败不算切换失败，只落日志。
    let started_client = start_client(key);
    if started_client {
        message.push_str(if closed {
            "，客户端已重新启动"
        } else {
            "，已打开客户端"
        });
    }

    // 收尾必须写清「现在停在什么状态」：切完到底登录了谁，出问题时这是第一手信息
    log::item(
        log::Item::new("切换账号", &subject, log::Outcome::Ok)
            .reason("已完成")
            .evidence(format!(
                "当前登录={subject} | 客户端={} | 原账号={}",
                if started_client {
                    "已启动"
                } else {
                    "未启动"
                },
                previous.as_deref().unwrap_or("-")
            ))
            .ms(started.elapsed().as_millis() as u64),
    );
    crate::app::log::log(
        crate::app::log::Level::Info,
        "accounts",
        // 日志用昵称，不出现完整 iid（uid 只走「复制 UID」）
        format!("切换登录：{} → {message}", identity.name),
    );
    Ok(SwitchResult { message })
}

/// 用 WorkBuddy **国内版**账号的档案切换 CodeBuddy 家族客户端（软件 / IDEA 插件）的登录。
///
/// 三端同号、但切换分开做：这个入口只动目标客户端的凭证落点，不碰 WorkBuddy
/// 自己的登录态（那走上面的 `switch`）。
///
/// 只支持国内版：CodeBuddy 系两个客户端都是国内产品线，国际版账号的票据在它们
/// 那边没有可用的授权域，写进去只会得到一份「登不上」的会话——所以国际版直接拒绝，
/// 界面也不再给入口。
pub fn switch_codebuddy(db: &Db, roots: &Roots, iid: &str, target: &str) -> Result<SwitchResult> {
    let identity = repo::identity(db, iid)?
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "账号不存在，可能已被删除"))?;
    if identity.platform != "workbuddy" {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "只有 WorkBuddy 账号支持切换 CodeBuddy 系客户端",
        ));
    }
    let edition = edition_of(&identity.account_id);
    if edition == "intl" {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "国际版账号不支持切换 CodeBuddy 系客户端（只支持国内版）",
        ));
    }
    let target = providers::codebuddy::Target::parse(target)?;
    let uid = uid_of(&identity.account_id);
    let outcome = providers::codebuddy::switch_target(roots, uid, &identity.name, edition, target)?;
    crate::app::log::log(
        crate::app::log::Level::Info,
        "accounts",
        format!(
            "切换 CodeBuddy 系登录：{} → {}",
            identity.name, outcome.message
        ),
    );
    Ok(SwitchResult {
        message: outcome.message,
    })
}

/// 拉起客户端（切换前关过、或切换完该以新账号进入）。
/// 找不到 exe 或启动失败都只落日志，不影响「登录态已经切好」这个结论。
fn start_client(key: &str) -> bool {
    let Some(exe) = super::add::client_exe(key) else {
        crate::app::log::log(
            crate::app::log::Level::Warn,
            "accounts",
            format!("没找到 {key} 的客户端安装位置，请手动打开"),
        );
        return false;
    };
    match process::start_detached(&exe) {
        Ok(()) => true,
        Err(e) => {
            crate::app::log::log(
                crate::app::log::Level::Warn,
                "accounts",
                format!("启动客户端失败：{}", e.message),
            );
            false
        }
    }
}

/// 这个账号是不是客户端当前登录的账号。读不到（没装/没登录）一律 false。
pub fn is_current(db: &Db, iid: &str) -> Result<bool> {
    let identity =
        repo::identity(db, iid)?.ok_or_else(|| AppError::new(ErrorKind::Invalid, "账号不存在"))?;
    let Some(provider) = providers::for_iid(iid) else {
        return Ok(false);
    };
    Ok(provider.current_id().as_deref() == Some(identity.account_id.as_str()))
}

/// 各客户端**当前**登录的账号 iid 列表。界面用它给行标「当前」。
/// 一次遍历六家，比每行各读一次客户端文件便宜得多。
pub fn current_iids() -> Vec<String> {
    let mut out = Vec::new();
    for provider in providers::all() {
        let Some(id) = provider.current_id() else {
            continue;
        };
        // WorkBuddy 的 current_id 已经带了 @intl 后缀，其余平台直接拼
        out.push(format!("{}:{id}", provider.platform()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::accounts::AccountIdentity;

    fn seed(db: &Db, iid: &str) -> AccountIdentity {
        let (platform, account_id) = iid.split_once(':').unwrap();
        let identity = AccountIdentity {
            iid: iid.into(),
            platform: platform.into(),
            platform_label: "Test".into(),
            account_id: account_id.into(),
            name: "测试".into(),
            requires_checkin: true,
            ..Default::default()
        };
        repo::upsert_identity(db, &identity, 1).unwrap();
        identity
    }

    fn roots(dir: &tempfile::TempDir) -> Roots {
        Roots::at(dir.path(), dir.path(), dir.path())
    }

    #[test]
    fn unknown_account_is_an_invalid_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        let err = switch(&db, &roots(&dir), "qoder:none").unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Invalid));
    }

    #[test]
    fn account_without_secret_is_refused_before_touching_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "workbuddy:u1");
        let err = switch(&db, &roots(&dir), "workbuddy:u1").unwrap_err();
        assert_eq!(err.message, "这个账号没有保存登录凭据，无法切换");
    }

    #[test]
    fn corrupt_secret_is_reported_not_panicked() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "qoder:u1");
        repo::set_secret(&db, "qoder:u1", b"not-json", None).unwrap();
        let err = switch(&db, &roots(&dir), "qoder:u1").unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Invalid));
    }

    /// 国际版账号：在碰任何客户端文件之前就拒绝（CodeBuddy 系只服务国内版）。
    #[test]
    fn intl_accounts_are_refused_before_touching_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "workbuddy:u1@intl");
        let err = switch_codebuddy(&db, &roots(&dir), "workbuddy:u1@intl", "software").unwrap_err();
        assert!(matches!(err.kind, crate::app::error::ErrorKind::Invalid));
        assert!(err.message.contains("国际版"), "{}", err.message);
    }

    #[test]
    fn is_current_is_false_when_client_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open_at(&dir.path().join("app.db")).unwrap();
        seed(&db, "trae:u1");
        // Trae 客户端目录由环境变量决定；无论如何不该 panic，也不该说「是当前」
        assert!(!is_current(&db, "trae:u1").unwrap());
    }
}
