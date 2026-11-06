//! IPC 命令面：按能力域分文件，名单集中在 desktop/mod.rs 的 generate_handler!。
//! 每个文件只做转接（AGENTS §4），业务在 app/ 对应模块。
//!
//! 这里显式再导 desktop 的私有 use：`use super::*` 的 glob 取不到父模块的
//! 私有 use 绑定，各域文件的 `use super::*` 依赖本文件的再导出。
pub(crate) use super::{err_text, execute_schedule, AppState};
pub(crate) use crate::app::error::AppError;
pub(crate) use crate::app::log::{self, Level};
pub(crate) use tauri::State;

pub mod accounts;
pub mod activity;
pub mod checkin;
pub mod hub;
pub mod logs;
pub mod maintenance;
pub mod schedule;
pub mod sessions;
pub mod settings;
pub mod shell;
pub mod system;
pub mod usage;
