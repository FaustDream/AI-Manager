//! Trae 用量：客户端主库 `database.db` 是加密格式，明文读不到 token。
//!
//! 插件侧 `.marscode` 只有会话正文，没有 usage 字段；因此不采集，只在覆盖表里
//! 如实说明。Trae 的积分走 `providers/trae.rs` 的签到前后余额对比，与本地 token
//! 采集是两件事。
use super::SourceCoverage;

pub(super) fn coverage() -> Vec<SourceCoverage> {
    vec![SourceCoverage {
        platform: "trae".into(),
        label: "Trae".into(),
        level: "none".into(),
        note: "本地 database.db 为加密格式，明文读不到".into(),
    }]
}
