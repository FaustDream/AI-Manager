//! 平台端点与协议常量。**这些不是运行配置**：URL 与 UA 改动会被服务端
//! 风控/版本校验盯上（实测 UA 不对会被直接拒绝），因此写死在代码里。

pub const API_CN: &str = "https://copilot.tencent.com/v2/billing/meter";
pub const API_INTL: &str = "https://www.workbuddy.ai/v2/billing/meter";
pub const USER_AGENT: &str = "Mozilla/5.0 CodeBuddy/5.6.0";

/// 猫猫旅行走 Web 域名：实测 `www.workbuddy.cn` 与 `copilot.tencent.com`
/// 都接受同一份 Bearer 令牌，这里固定用文档里的 Web 域名，减少一处漂移。
pub const TRAVEL_BASE: &str = "https://www.workbuddy.cn";
pub const TRAVEL_INFO: &str = "/activity/growth/buddy/info";
pub const TRAVEL_CONFIG: &str = "/activity/growth/buddy/travel/config";
pub const TRAVEL_STATUS: &str = "/activity/growth/buddy/travel/status";
pub const TRAVEL_RECORDS: &str = "/activity/growth/buddy/travel/records";
pub const TRAVEL_CLAIM: &str = "/activity/growth/buddy/travel/claim";
pub const TRAVEL_DEPART: &str = "/activity/growth/buddy/travel/depart";

/// 成长中心（个人中心 /profile/growth-center 背后的接口）。
/// 任务列表在 `/v2` 下，其余在 `/activity/growth` 下 —— 以 Web 端 bundle 里的
/// 实际路径为准，不要「统一加前缀」。
pub const CORE_BASE: &str = "https://copilot.tencent.com";

pub const GROWTH_TASKS: &str = "/v2/activity/growth/tasks";
pub const GROWTH_HEATMAP: &str = "/activity/growth/heatmap";
pub const GROWTH_STREAK: &str = "/activity/growth/streak";
