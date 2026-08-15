mod model;
mod repository;
mod schema;

use std::path::PathBuf;

use tauri::State;

pub(crate) use model::{
    CreateArtworkRequest, LibrarySearchResult, LibraryTrashEntry, LibraryTree,
    MoveLibraryNodesRequest, RepositoryStatus,
};
#[cfg(test)]
pub(crate) use repository::create_artwork;
pub(crate) use repository::{
    check_existing, create_artwork_and_list, empty_trash, initialize, open_existing,
    permanently_delete_trash,
};
#[cfg(test)]
pub(crate) use schema::take_integrity_check_count;

use crate::app::AppState;

fn ready_root(state: &AppState) -> Result<PathBuf, String> {
    state.ready_repository_path()
}

#[tauri::command]
pub(crate) fn get_repository_status(
    state: State<'_, AppState>,
) -> Result<RepositoryStatus, String> {
    let Some(root) = state.repository_path()? else {
        return Ok(RepositoryStatus {
            configured: false,
            ready: false,
            root_path: String::new(),
            database_path: String::new(),
            error: None,
        });
    };
    let database = repository::database_path(&root);
    match state.ready_repository_path() {
        Ok(ready_root) => Ok(RepositoryStatus {
            configured: true,
            ready: true,
            root_path: ready_root.to_string_lossy().into_owned(),
            database_path: database.to_string_lossy().into_owned(),
            error: None,
        }),
        Err(error) => Ok(RepositoryStatus {
            configured: true,
            ready: false,
            root_path: root.to_string_lossy().into_owned(),
            database_path: database.to_string_lossy().into_owned(),
            error: Some(error),
        }),
    }
}

#[tauri::command]
pub(crate) fn list_library_tree(state: State<'_, AppState>) -> Result<LibraryTree, String> {
    repository::list_tree(&ready_root(state.inner())?)
}

#[tauri::command]
pub(crate) fn search_library(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<LibrarySearchResult>, String> {
    repository::search(&ready_root(state.inner())?, &query)
}

#[tauri::command]
pub(crate) fn create_library_group(
    state: State<'_, AppState>,
    parent_id: Option<String>,
    title: String,
) -> Result<LibraryTree, String> {
    repository::create_group(&ready_root(state.inner())?, parent_id.as_deref(), &title)
}

#[tauri::command]
pub(crate) fn rename_library_node(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<LibraryTree, String> {
    repository::rename_node(&ready_root(state.inner())?, &id, &title)
}

#[tauri::command]
pub(crate) fn trash_library_nodes(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<LibraryTree, String> {
    repository::trash_nodes(&ready_root(state.inner())?, &ids)
}

#[tauri::command]
pub(crate) fn list_library_trash(
    state: State<'_, AppState>,
) -> Result<Vec<LibraryTrashEntry>, String> {
    repository::list_trash(&ready_root(state.inner())?)
}

#[tauri::command]
pub(crate) fn restore_library_trash(
    state: State<'_, AppState>,
    id: String,
) -> Result<LibraryTree, String> {
    repository::restore_trash(&ready_root(state.inner())?, &id)
}

#[tauri::command]
pub(crate) fn move_library_nodes(
    state: State<'_, AppState>,
    request: MoveLibraryNodesRequest,
) -> Result<LibraryTree, String> {
    repository::move_nodes(&ready_root(state.inner())?, request)
}
