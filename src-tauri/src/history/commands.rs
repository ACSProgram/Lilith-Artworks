use tauri::State;

use crate::app::AppState;

use super::{ArtworkHistory, RenameHistoryNodeRequest};

#[tauri::command]
pub(crate) fn get_artwork_history(
    artwork_id: String,
    state: State<'_, AppState>,
) -> Result<ArtworkHistory, String> {
    state.with_ready_repository(|root| super::list(root, &artwork_id))
}

#[tauri::command]
pub(crate) fn rename_history_node(
    request: RenameHistoryNodeRequest,
    app_state: State<'_, AppState>,
) -> Result<ArtworkHistory, String> {
    app_state.with_ready_repository(|root| {
        let node = super::load_node(root, &request.history_id)?;
        super::rename_node(root, &request.history_id, &request.title)?;
        super::list(root, &node.artwork_id)
    })
}
