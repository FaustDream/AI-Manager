//! 豆包用量：客户端会话 Token 加密存储，读不到逐次用量。
//!
//! 不采集 token；额度以签到面板（`providers/doubao.rs`）为准。覆盖表里标为
//! `none`，避免把「没有数据」误解成「没有统计」。
use super::SourceCoverage;

pub(super) fn coverage() -> Vec<SourceCoverage> {
    vec![SourceCoverage {
        platform: "doubao".into(),
        label: "豆包".into(),
        level: "none".into(),
        note: "客户端会话 Token 加密存储，不采集；额度以签到面板为准".into(),
    }]
}
