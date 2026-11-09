// activity 的 IPC 命令：只做转接，编排在 app/activity。

use super::*;

/// 详情卡一次取齐：状态、地点、记录、账号偏好。
#[tauri::command]
pub async fn activity_overview(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::activity::ActivityOverview, String> {
    state.activity.overview(&iid).await.map_err(err_text)
}

/// 领取旅行奖励（服务端幂等；只对「已回来」状态有效）。
#[tauri::command]
pub async fn activity_claim(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::types::ActionResult, String> {
    state.activity.claim(&iid, "user").await.map_err(err_text)
}

/// 派出猫猫；不传地点时用账号偏好，没配就取地点列表第一个。
#[tauri::command]
pub async fn activity_depart(
    state: State<'_, AppState>,
    iid: String,
    location: Option<i64>,
) -> std::result::Result<crate::types::ActionResult, String> {
    state
        .activity
        .depart(&iid, location, "user")
        .await
        .map_err(err_text)
}

/// 当日活跃明细（只读）：活跃地图 + 连续天数 + 未完成任务。
#[tauri::command]
pub async fn activity_daily_status(
    state: State<'_, AppState>,
    iid: String,
) -> std::result::Result<crate::app::activity::DailyActive, String> {
    state.activity.daily_status(&iid).await.map_err(err_text)
}

/// 保存账号级旅行偏好（是否参与自动旅行 + 地点）。
#[tauri::command]
pub async fn activity_pref_set(
    state: State<'_, AppState>,
    iid: String,
    enabled: bool,
    location: Option<i64>,
) -> std::result::Result<crate::app::activity::TravelPrefs, String> {
    state
        .activity
        .set_prefs(&iid, enabled, location)
        .await
        .map_err(err_text)
}
