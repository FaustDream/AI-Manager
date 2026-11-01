//! 基础设施层：业务模块只准依赖这里，不得直接读系统时钟、网络或真实路径。
//!
//! 这一层存在的唯一理由：让契约可测。v1 里 `datetime.now()`、`requests.get()`、
//! 直接拼 `Path.home()` 散落在业务代码中，导致「明天能不能签」「同一天刷两次会不会
//! 产生 0 增减」这类边界根本无法构造。

pub mod accounts;
pub mod checkin;
pub mod clients;
pub mod clock;
pub mod db;
pub mod error;
pub mod fsx;
pub mod hub;
pub mod log;
pub mod maintenance;
pub mod migrate;
pub mod net;
pub mod paths;
pub mod platform;
pub mod process;
pub mod providers;
pub mod runtime;
pub mod secret;
pub mod sessions;
pub mod task;
pub mod timeutil;

use crate::app::clock::Clock;

/// 传给每个业务函数的上下文：时钟、网络、数据目录、取消信号都从这里取。
/// 业务代码不得绕过它直接访问系统资源。
pub struct TaskCtx<'a> {
    pub clock: &'a dyn Clock,
    pub http: &'a dyn net::Http,
    pub data_dir: &'a std::path::Path,
}

impl<'a> TaskCtx<'a> {
    pub fn new(
        clock: &'a dyn Clock,
        http: &'a dyn net::Http,
        data_dir: &'a std::path::Path,
    ) -> Self {
        Self {
            clock,
            http,
            data_dir,
        }
    }
}
