//! Qoder 用量：本地只有 `chat_session_context_usage` 的上下文占用快照。
//!
//! 快照是「当前上下文窗口用了多少」，不是逐次调用量，也没有扣费字段；写进
//! `token_usage_records` 会被当成一次调用，既不能按天聚合也不能算钱，因此不采集，
//! 只在覆盖表里如实说明。
use super::SourceCoverage;

pub(super) fn coverage() -> Vec<SourceCoverage> {
    vec![SourceCoverage {
        platform: "qoder".into(),
        label: "Qoder".into(),
        level: "context".into(),
        note: "只有上下文占用快照，没有逐次 token / 计费".into(),
    }]
}
