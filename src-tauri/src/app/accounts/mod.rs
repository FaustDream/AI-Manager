//! 账号中心的数据层。
//!
//! 两块：`view` 负责「一行一个视图 + 合并规则」，`repo` 负责 SQL 与凭据边界。
//! 扫描与切换登录（Provider / switcher）在 P2 后续接入，不放这里。

pub mod repo;
pub mod view;

pub use view::{AccountIdentity, AccountTable, AccountView};
