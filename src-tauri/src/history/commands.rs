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
pub(crate) async fn get_history_node_format(
    history_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Option<String>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::node_format(context, &history_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn get_history_node_thumbnail(
    history_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<super::NodePreview, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::thumbnail(context, &history_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn refresh_history_node_thumbnail(
    history_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Option<String>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::History, move |context| {
            usecases::history::refresh_thumbnail(context, &history_id)
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
