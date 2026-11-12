//! 账号中心的数据层。
//!
//! 四块：`view` 一行一个视图 + 合并规则；`repo` SQL 与凭据边界；`credential`
//! 加密存放的凭据包；`scan` 扫描编排（Provider 在 `app/providers/`，一家一个文件）。
//! 切换登录在 `app/providers/*::switch_to`，编排在 `switch`。

pub mod add;
pub mod credential;
pub mod login;
pub mod profile;
pub mod refresh;
pub mod renew;
pub mod repo;
pub mod scan;
pub mod switch;
pub mod view;

use crate::app::db::Db;
use crate::app::error::{AppError, ErrorKind, Result};
use crate::app::runtime::blocking;
use std::sync::Arc;

/// 取账号视图与凭据包。**唯一入口**：签到、旅行、诊断都走这里，
/// 避免每处各写一遍「找不到账号 / 没保存凭据」的错误话术。
/// 凭据只在这一次调用里存在，不往外传。
pub async fn load_credential(
    db: Arc<Db>,
    iid: &str,
) -> Result<(view::AccountView, credential::Credential)> {
    let wanted = iid.to_string();
    blocking(move || {
        let view = repo::list(&db)?
            .into_iter()
            .find(|v| v.iid == wanted)
            .ok_or_else(|| AppError::new(ErrorKind::Invalid, "账号不存在，可能已被删除"))?;
        let bytes = repo::secret_of(&db, &wanted)?.ok_or_else(|| {
            AppError::new(
                ErrorKind::Invalid,
                "这个账号没有保存凭据，请重新点「扫描客户端」",
            )
        })?;
        Ok((view, credential::Credential::parse(&bytes)?))
    })
    .await?
}

pub use refresh::{refresh_all, RefreshReport};
pub use renew::{renew, RenewOutcome, RenewState};
pub use scan::{scan_all, ScanReport};
pub use switch::{current_iids, is_current, switch, switch_codebuddy, SwitchResult};
pub use view::{AccountIdentity, AccountTable, AccountView};

/// WorkBuddy 国内/国际共用 `workbuddy` 平台前缀，靠 account_id 的 `@intl` 后缀区分。
/// 参数可以是完整 iid 或纯 account_id，两者都以该后缀结尾。
pub fn is_intl_account(key: &str) -> bool {
    key.ends_with("@intl")
}

/// 去掉 `@intl` 后的真实 uid；其余账号原样返回。
pub fn uid_of(key: &str) -> &str {
    key.trim_end_matches("@intl")
}

/// 档案/客户端版本目录名：`cn` / `intl`。
pub fn edition_of(key: &str) -> &'static str {
    if is_intl_account(key) {
        "intl"
    } else {
        "cn"
    }
}
