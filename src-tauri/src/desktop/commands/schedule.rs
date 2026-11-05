// schedule 的 IPC 命令：只做转接，业务在 app/schedule。

use super::*;
use crate::app::error::ErrorKind;
use crate::app::schedule::Schedule;

/// 全部定时任务（首次读取会写入默认种子）。
#[tauri::command]

pub async fn schedule_list(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<Schedule>, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::schedule::list(&db))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 新建或更新一条。规则校验（时刻格式、间隔范围）在后端做，绕开界面也写不进坏配置。
#[tauri::command]

pub async fn schedule_save(
    state: State<'_, AppState>,
    input: Schedule,
) -> std::result::Result<Schedule, String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::schedule::save(&db, input))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

#[tauri::command]

pub async fn schedule_delete(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<(), String> {
    let db = state.db.clone();
    crate::app::runtime::blocking(move || crate::app::schedule::delete(&db, &id))
        .await
        .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
        .map_err(err_text)
}

/// 立即执行一次（不看规则）。手动签到与「这条定时到底能不能跑」的验证都走它，
/// 结果同样落一条 `trigger=user` 的执行记录——手动和定时在记录里必须分得清。
#[tauri::command]

pub async fn schedule_run(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: String,
) -> std::result::Result<String, String> {
    let db = state.db.clone();
    let service = state.checkin.clone();
    let schedule = crate::app::runtime::blocking({
        let db = db.clone();
        let id = id.clone();
        move || crate::app::schedule::list(&db).map(|list| list.into_iter().find(|s| s.id == id))
    })
    .await
    .map_err(|e| err_text(AppError::new(ErrorKind::Io, e.to_string())))?
    .map_err(err_text)?
    .ok_or_else(|| err_text(AppError::new(ErrorKind::Invalid, "这条定时任务不存在")))?;

    let started = std::time::Instant::now();
    // 与定时触发走同一条执行路径，只在记录里区分 trigger
    let (ok, message) = execute_schedule(&service, &app, &schedule).await;
    // 手动跑也要更新 last_run：否则「每 N 小时」的任务刚手动跑完，下一跳又自动跑一次
    let _ = crate::app::runtime::blocking(move || crate::app::schedule::mark_ran(&db, &id)).await;
    crate::app::log::action(
        &format!("schedule.{}", schedule.id),
        "user",
        ok,
        started.elapsed().as_millis() as u64,
        format!("手动执行「{}」：{message}", schedule.name),
    );
    Ok(message)
}
