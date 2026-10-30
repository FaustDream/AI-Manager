//! 账号中心的数据层。
//!
//! 三块：`view` 一行一个视图 + 合并规则；`repo` SQL 与凭据边界；
//! `scan` 扫描编排（Provider 在 `app/providers/`，一家一个文件）。
//! 切换登录（switcher）在 P2 后续接入。

pub mod repo;
pub mod scan;
pub mod view;

pub use scan::{scan_all, ScanReport};
pub use view::{AccountIdentity, AccountTable, AccountView};
