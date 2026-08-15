use tauri::State;

use crate::app::AppState;

use super::{ArtworkHistory, RenameHistoryNodeRequest};

fn root(state: &AppState) -> Result<std::path::PathBuf, String> {
    state.ready_repository_path()
}

#[tauri::command]
pub(crate) fn get_artwork_history(
    artwork_id: String,
    state: State<'_, AppState>,
) -> Result<ArtworkHistory, String> {
    super::list(&root(state.inner())?, &artwork_id)
}

#[tauri::command]
pub(crate) fn rename_history_node(
    request: RenameHistoryNodeRequest,
    app_state: State<'_, AppState>,
) -> Result<ArtworkHistory, String> {
    let root = root(app_state.inner())?;
    let node = super::load_node(&root, &request.history_id)?;
    super::rename_node(&root, &request.history_id, &request.title)?;
    super::list(&root, &node.artwork_id)
}
