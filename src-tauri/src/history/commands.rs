use tauri::State;

use crate::app::AppState;

use super::{ArtworkHistory, RenameHistoryNodeRequest};

#[tauri::command]
pub(crate) async fn get_artwork_history(
    artwork_id: String,
    state: State<'_, AppState>,
) -> Result<ArtworkHistory, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_repository_read(|root| super::list(root, &artwork_id))
    })
    .await
    .map_err(|error| format!("历史读取任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn rename_history_node(
    request: RenameHistoryNodeRequest,
    app_state: State<'_, AppState>,
) -> Result<ArtworkHistory, String> {
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        app_state.with_ready_repository(|root| {
            let node = super::load_node(root, &request.history_id)?;
            super::rename_node(root, &request.history_id, &request.title)?;
            super::list(root, &node.artwork_id)
        })
    })
    .await
    .map_err(|error| format!("历史节点重命名任务异常结束：{error}"))?
}
