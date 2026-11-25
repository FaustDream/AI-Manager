// dida 的 IPC 命令：只做转接，业务在 app/dida。

use super::*;

pub async fn dida_state(state: &AppState) -> crate::app::dida::DidaState {
    state.dida.state().await
}

pub async fn dida_token_set(
    state: &AppState,
    token: String,
) -> std::result::Result<crate::app::dida::DidaState, String> {
    state.dida.set_token(&token).await.map_err(err_text)
}

pub async fn dida_token_clear(
    state: &AppState,
) -> std::result::Result<crate::app::dida::DidaState, String> {
    state.dida.clear_token().await.map_err(err_text)
}

// 转接层参数与 IPC 字段一一对应，不另造承载结构。
pub async fn dida_task_create(
    state: &AppState,
    title: String,
    project_id: String,
    content: Option<String>,
    due_date: Option<String>,
    priority: Option<u8>,
    is_all_day: Option<bool>,
) -> std::result::Result<crate::app::dida::DidaTask, String> {
    state
        .dida
        .create_task(
            &title,
            &project_id,
            content.as_deref(),
            due_date.as_deref(),
            priority,
            is_all_day.unwrap_or(false),
        )
        .await
        .map_err(err_text)
}

pub async fn dida_tasks(
    state: &AppState,
    project_id: String,
) -> std::result::Result<Vec<crate::app::dida::DidaTask>, String> {
    state.dida.list_tasks(&project_id).await.map_err(err_text)
}

#[allow(clippy::too_many_arguments)]
pub async fn dida_task_update(
    state: &AppState,
    task_id: String,
    project_id: String,
    title: String,
    content: Option<String>,
    due_date: Option<String>,
    priority: Option<u8>,
    is_all_day: Option<bool>,
) -> std::result::Result<crate::app::dida::DidaTask, String> {
    state
        .dida
        .update_task(
            &task_id,
            &project_id,
            &title,
            content.as_deref(),
            due_date.as_deref(),
            priority,
            is_all_day.unwrap_or(false),
        )
        .await
        .map_err(err_text)
}

pub async fn dida_task_complete(
    state: &AppState,
    project_id: String,
    task_id: String,
) -> std::result::Result<(), String> {
    state
        .dida
        .complete_task(&project_id, &task_id)
        .await
        .map_err(err_text)
}

pub async fn dida_task_delete(
    state: &AppState,
    project_id: String,
    task_id: String,
) -> std::result::Result<(), String> {
    state
        .dida
        .delete_task(&project_id, &task_id)
        .await
        .map_err(err_text)
}
