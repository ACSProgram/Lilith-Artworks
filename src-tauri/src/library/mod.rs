mod model;
mod repository;

use std::path::{Path, PathBuf};

use tauri::State;

pub(crate) use model::{
    CreateArtworkRequest, LibrarySearchResult, LibraryTrashEntry, LibraryTree,
    MoveLibraryNodesRequest, RepositoryStatus,
};
#[cfg(test)]
pub(crate) use repository::create_artwork;
pub(crate) use repository::initialize;

use crate::{app::AppState, backup::BackupState, cleanup};

fn ready_root(state: &AppState) -> Result<PathBuf, String> {
    let root = state.repository_path()?.ok_or("尚未配置作品仓库")?;
    initialize(&root)?;
    Ok(root)
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
    match initialize(Path::new(&root)) {
        Ok(()) => Ok(RepositoryStatus {
            configured: true,
            ready: true,
            root_path: root.to_string_lossy().into_owned(),
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
pub(crate) fn create_library_artwork(
    state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    request: CreateArtworkRequest,
) -> Result<LibraryTree, String> {
    let tree = repository::create_artwork_and_list(
        &ready_root(state.inner())?,
        request.parent_id.as_deref(),
        &request.title,
        &request.branch_title,
        Path::new(&request.source_path),
    )?;
    backup_state.wake_scheduler();
    Ok(tree)
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
pub(crate) async fn permanently_delete_library_trash(
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    ids: Vec<String>,
) -> Result<cleanup::CleanupReport, String> {
    let root = ready_root(app_state.inner())?;
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = state.run_exclusive(None, || {
            let cleanup_ids = repository::permanently_delete_trash(&root, &ids)?;
            cleanup::run(&root, &cleanup_ids)
        })?;
        state.wake_scheduler();
        Ok(report)
    })
    .await
    .map_err(|error| format!("永久删除任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn empty_library_trash(
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<cleanup::CleanupReport, String> {
    let root = ready_root(app_state.inner())?;
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = state.run_exclusive(None, || {
            let cleanup_ids = repository::empty_trash(&root)?;
            cleanup::run(&root, &cleanup_ids)
        })?;
        state.wake_scheduler();
        Ok(report)
    })
    .await
    .map_err(|error| format!("清空回收站任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) fn move_library_nodes(
    state: State<'_, AppState>,
    request: MoveLibraryNodesRequest,
) -> Result<LibraryTree, String> {
    repository::move_nodes(&ready_root(state.inner())?, request)
}
