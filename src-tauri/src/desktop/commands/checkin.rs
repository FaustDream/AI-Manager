// checkin 的 IPC 命令：只做转接，业务在 app/ 对应模块。

use super::*;
use crate::app::error::ErrorKind;

// ------------------------------------------------------------ 签到与积分

/// 「刷新积分」：刷余额，需要签到的平台顺带刷签到状态（否则平台侧已签的账号
/// 会一直停在「待查询」）。真正的并发与失败兜底在 `Service::credits_batch`。
#[tauri::command]
pub async fn checkin_credits(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<Vec<(String, crate::types::CreditSnapshot)>, String> {
    // 并发与失败兜底都在 Service::credits_batch 里，命令层只转接
    Ok(state.checkin.credits_batch(&iids).await)
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
    let result = service.checkin(&iid).await.map_err(err_text)?;
    log::log(
        if result.ok { Level::Info } else { Level::Warn },
        "checkin",
        format!(
            "{}：{}",
            iid.split(':').next().unwrap_or("-"),
            result.message
        ),
    );
    Ok(result)
}

/// 批量签到：任务化，可取消，并发 3。
#[tauri::command]

pub async fn checkin_batch(
    state: State<'_, AppState>,
    iids: Vec<String>,
) -> std::result::Result<u64, String> {
    use crate::app::task::CapabilityId;

    let total = iids.len();
    let service = state.checkin.clone();
    let task_id = state.tasks.spawn_managed(
        CapabilityId::AccountCenter,
        format!("批量签到 {total} 个账号"),
        move |handle| async move {
            let cancel = handle.cancel.clone();
            let round = service
                .batch(&iids, cancel, |done, total| {
                    handle.progress(done as f32 / total.max(1) as f32, format!("{done}/{total}"));
                })
                .await;
            let message = round.message.clone();
            handle.message(message.clone());
            log::log(Level::Info, "checkin", format!("批量签到结束：{message}"));
            // 结束态交给统一归类：全失败记 Failed、部分失败记 Partial、取消记 Cancelled
            if handle.cancel.is_cancelled() {
                Err(crate::app::error::AppError::new(
                    ErrorKind::Cancelled,
                    message,
                ))
            } else if round.failed > 0 && round.success == 0 {
                Err(crate::app::error::AppError::new(ErrorKind::Io, message))
            } else if round.failed > 0 {
                Err(crate::app::error::AppError::new(
                    ErrorKind::Conflict,
                    message,
                ))
            } else {
                Ok(())
            }
        },
    );
    Ok(task_id)
}

/// 每日轮询的最近一轮结果（界面底部那一行）。
#[tauri::command]

pub fn daily_state(state: State<'_, AppState>) -> Option<crate::app::checkin::RoundResult> {
    state.checkin.daily_state()
}

/// 手动跑一轮（会在后台执行，返回任务 id）。
#[tauri::command]

pub async fn daily_run_now(state: State<'_, AppState>) -> std::result::Result<u64, String> {
    use crate::app::task::CapabilityId;

    let service = state.checkin.clone();
    let task_id = state.tasks.spawn_managed(
        CapabilityId::AccountCenter,
        "每日轮询（手动）",
        move |handle| async move {
            let round = service.run_round(&handle.cancel).await;
            log::log(
                if round.failed > 0 {
                    Level::Warn
                } else {
                    Level::Info
                },
                "checkin",
                format!("每日轮询：{}", round.message),
            );
            handle.message(round.message.clone());
            if handle.cancel.is_cancelled() {
                Err(crate::app::error::AppError::new(
                    ErrorKind::Cancelled,
                    round.message,
                ))
            } else {
                Ok(())
            }
        },
    );
    Ok(task_id)
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
