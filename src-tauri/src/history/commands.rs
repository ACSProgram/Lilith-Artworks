use tauri::State;

use crate::app::{command_runtime::OperationDomain, usecases, OperationCoordinator};

use super::{ArtworkHistory, RenameHistoryNodeRequest};

#[tauri::command]
pub(crate) async fn get_artwork_history(
    artwork_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<ArtworkHistory, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::list(context, &artwork_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn rename_history_node(
    request: RenameHistoryNodeRequest,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<ArtworkHistory, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::rename_node(context, request)
        })
        .await
        .map_err(String::from)
}
