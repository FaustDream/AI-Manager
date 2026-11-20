//! 基础设施层：业务模块只准依赖这里，不得直接读系统时钟、网络或真实路径。
//!
//! 这一层存在的唯一理由：让契约可测。把时钟、网络与真实路径收敛到这里，
//! 业务代码不直接读，才能让「明天能不能签」「同一天刷两次会不会
//! 产生 0 增减」这类边界可构造。

pub mod accounts;
pub mod activity;
pub mod browser;
pub mod capability;
pub mod checkin;
pub mod clients;
pub mod clock;
pub mod config;
pub mod config_history;
pub mod db;
pub mod error;
pub mod events;
pub mod fsx;
pub mod health;
pub mod hub;
pub mod log;
pub mod maintenance;
pub mod model_api;
pub mod net;
pub mod paths;
pub mod platform;
pub mod process;
pub mod products;
pub mod progress;
pub mod providers;
pub mod runtime;
pub mod schedule;
pub mod secret;
pub mod selfcheck;
pub mod sessions;
pub mod timeutil;
pub mod update;
pub mod usage;
pub mod window;
