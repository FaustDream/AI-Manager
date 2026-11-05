//! 账号中心的数据层。
//!
//! 四块：`view` 一行一个视图 + 合并规则；`repo` SQL 与凭据边界；`credential`
//! 加密存放的凭据包；`scan` 扫描编排（Provider 在 `app/providers/`，一家一个文件）。
//! 切换登录在 `app/providers/*::switch_to`，编排在 `switch`。

pub mod add;
pub mod credential;
pub mod login;
pub mod profile;
pub mod repo;
pub mod scan;
pub mod switch;
pub mod view;

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
