mod model;
mod repository;
mod schema;

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

#[tauri::command]
pub(crate) async fn get_repository_status(
    state: State<'_, AppState>,
) -> Result<RepositoryStatus, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
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
    })
    .await
    .map_err(|error| format!("仓库状态检查任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn list_library_tree(state: State<'_, AppState>) -> Result<LibraryTree, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.with_repository_read(repository::list_tree))
        .await
        .map_err(|error| format!("作品树读取任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn search_library(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<LibrarySearchResult>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_repository_read(|root| repository::search(root, &query))
    })
    .await
    .map_err(|error| format!("作品搜索任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn create_library_group(
    state: State<'_, AppState>,
    parent_id: Option<String>,
    title: String,
) -> Result<LibraryTree, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| {
            repository::create_group(root, parent_id.as_deref(), &title)
        })
    })
    .await
    .map_err(|error| format!("创建分组任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn rename_library_node(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<LibraryTree, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| repository::rename_node(root, &id, &title))
    })
    .await
    .map_err(|error| format!("重命名任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn trash_library_nodes(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<LibraryTree, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| repository::trash_nodes(root, &ids))
    })
    .await
    .map_err(|error| format!("移到回收站任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn list_library_trash(
    state: State<'_, AppState>,
) -> Result<Vec<LibraryTrashEntry>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.with_repository_read(repository::list_trash))
        .await
        .map_err(|error| format!("回收站读取任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn restore_library_trash(
    state: State<'_, AppState>,
    id: String,
) -> Result<LibraryTree, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| repository::restore_trash(root, &id))
    })
    .await
    .map_err(|error| format!("回收站恢复任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn move_library_nodes(
    state: State<'_, AppState>,
    request: MoveLibraryNodesRequest,
) -> Result<LibraryTree, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_ready_repository(|root| repository::move_nodes(root, request))
    })
    .await
    .map_err(|error| format!("移动节点任务异常结束：{error}"))?
}
