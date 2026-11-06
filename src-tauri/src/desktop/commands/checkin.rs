// checkin 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;

// ------------------------------------------------------------ 签到与积分

/// 「刷新积分」：刷余额，需要签到的平台顺带刷签到状态（否则平台侧已签的账号
/// 会一直停在「待查询」）。真正的并发与失败兜底在 `Service::credits_batch`。
#[tauri::command]
pub async fn checkin_credits(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<Vec<(String, crate::types::CreditSnapshot)>, String> {
    // 并发、失败兜底与逐条失败日志都在 Service::credits_batch 里，命令层只转接
    Ok(state.checkin.credits_batch(&iids, "user").await)
}

/// 只读诊断：调平台查询接口，不写库、不触发签到；结果同时落日志 `diagnose`。
#[tauri::command]
pub async fn checkin_diagnose(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::checkin::DiagnoseReport, String> {
    state.checkin.diagnose(&iid).await.map_err(err_text)
}

/// 详情面板：余额、今日增减、批次、下次可签。只读本地库，不打接口。
#[tauri::command]

pub async fn checkin_detail(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::checkin::AccountDetail, String> {
    let service = state.checkin.clone();
    Ok(service.detail(&iid).await)
}

/// 单账号签到。**会真的调接口**，幂等由 daily_marks 保证。
#[tauri::command]

pub async fn checkin_run(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::types::ActionResult, String> {
    let service = state.checkin.clone();
    // 逐账号结果由 checkin_traced 落成动作行，命令层不再重复记一遍
    service.checkin(&iid).await.map_err(err_text)
}

/// 批量签到：并发 3，跑完返回本轮结果。
#[tauri::command]

pub async fn checkin_batch(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<crate::app::checkin::RoundResult, String> {
    let service = state.checkin.clone();
    // 批次开始/逐账号/批次结束三层记录都在 Service::batch 里
    Ok(service.batch(&iids, "user").await)
}

/// 每日轮询的最近一轮结果（界面底部那一行）。
#[tauri::command]

pub fn daily_state(state: State<'_, AppState>) -> Option<crate::app::checkin::RoundResult> {
    state.checkin.daily_state()
}

/// 手动跑一轮每日轮询，跑完返回本轮结果，并推一条 `checkin:round`。
/// 推事件的原因：顶部「跑一轮」与总览页都靠它统一失效账号列表，
/// 只在返回值里给结果的话，界面要点两次才看到新状态。
#[tauri::command]

pub async fn daily_run_now(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<crate::app::checkin::RoundResult, String> {
    use tauri::Emitter;
    let service = state.checkin.clone();
    // 轮询的批次记录已由 run_round 的 finish_batch 统一落盘
    let round = service.run_round("user").await;
    let _ = app.emit("checkin:round", &round);
    Ok(round)
}

/// ZCode 可领取的活动套餐（v1 的「领取」按钮第一步）。
#[tauri::command]

pub async fn zcode_claim_plans(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<Vec<crate::app::providers::zclaim::ClaimPlan>, String> {
    let service = state.checkin.clone();
    service.zcode_claimable(&iid).await.map_err(err_text)
}

/// 领取一个 ZCode 活动套餐：**会打开浏览器过验证码**，验证完自动提交。
#[tauri::command]

pub async fn zcode_claim_run(
    state: State<'_, AppState>,
    iid: String,
    plan_id: String,
) -> std::result::Result<crate::app::providers::zclaim::ClaimOutcome, String> {
    let service = state.checkin.clone();
    service.zcode_claim(&iid, &plan_id).await.map_err(err_text)
}

/// 积分趋势：读每日余额快照，不查接口。`iid` 为空表示全部账号汇总。
#[tauri::command]

pub async fn credit_trend(
    state: State<'_, AppState>,
    iid: Option<String>,
    days: Option<i64>,
) -> std::result::Result<Vec<crate::app::checkin::TrendPoint>, String> {
    Ok(state.checkin.credit_trend(iid, days.unwrap_or(30)).await)
}

/// 跨账号最近领取：仪表盘「最近领取」用。
#[tauri::command]

pub async fn credit_claims_recent(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> std::result::Result<Vec<crate::app::checkin::RecentClaim>, String> {
    Ok(state.checkin.recent_claims(limit.unwrap_or(20)).await)
}

/// 积分收支（按天）：到账 / 消耗 / 净额 / 当日末余额。
#[tauri::command]

pub async fn credit_flow(
    state: State<'_, AppState>,
    days: Option<i64>,
) -> std::result::Result<Vec<crate::app::checkin::CreditFlowPoint>, String> {
    Ok(state.checkin.credit_flow(days.unwrap_or(30)).await)
}
