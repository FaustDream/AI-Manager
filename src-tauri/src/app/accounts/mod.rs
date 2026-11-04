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
pub use switch::{current_iids, is_current, switch, SwitchResult};
pub use view::{AccountIdentity, AccountTable, AccountView};
