mod model;
mod repository;
mod schema;

use tauri::State;

#[cfg(feature = "headless")]
pub(crate) use model::CreatedArtwork;
/// 无头 `list-tree` 子命令需要递归遍历节点统计总数；产品命令与单元测试都不经这里引用，
/// 因此只在无头构建下再导出，否则 `cargo test` 会报未使用导入。
#[cfg(feature = "headless")]
pub(crate) use model::LibraryNode;
pub(crate) use model::{
    CreateArtworkRequest, LibrarySearchResult, LibraryTrashEntry, LibraryTree,
    MoveLibraryNodesRequest, RepositoryStatus,
};
/// 不带列表返回的创建入口：无头入口与 GUI use-case 共用同一实现。
#[cfg(any(test, feature = "headless"))]
pub(crate) use repository::create_artwork;
/// GUI 与 headless 共享的 use-case 入口。命令适配器只保留 Tauri 类型与错误映射。
pub(crate) use repository::{
    check_existing, create_artwork_and_list, create_group, database_path, empty_trash, initialize,
    list_trash, list_tree, move_nodes, open_existing, permanently_delete_trash, rename_node,
    restore_trash, search, trash_nodes,
};
#[cfg(test)]
pub(crate) use schema::take_integrity_check_count;

use crate::{app::command_runtime::OperationDomain, app::usecases, app::OperationCoordinator};

#[tauri::command]
pub(crate) async fn get_repository_status(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<RepositoryStatus, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Repository, |context| {
            usecases::library::repository_status(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn list_library_tree(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<LibraryTree, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, |context| {
            usecases::library::list_tree(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn search_library(
    coordinator: State<'_, OperationCoordinator>,
    query: String,
) -> Result<Vec<LibrarySearchResult>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, move |context| {
            usecases::library::search(context, query)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn create_library_group(
    coordinator: State<'_, OperationCoordinator>,
    parent_id: Option<String>,
    title: String,
) -> Result<LibraryTree, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, move |context| {
            usecases::library::create_group(context, parent_id, title)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn rename_library_node(
    coordinator: State<'_, OperationCoordinator>,
    id: String,
    title: String,
) -> Result<LibraryTree, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, move |context| {
            usecases::library::rename_node(context, id, title)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn trash_library_nodes(
    coordinator: State<'_, OperationCoordinator>,
    ids: Vec<String>,
) -> Result<LibraryTree, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, move |context| {
            usecases::library::trash_nodes(context, ids)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn list_library_trash(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Vec<LibraryTrashEntry>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, |context| {
            usecases::library::list_trash(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn restore_library_trash(
    coordinator: State<'_, OperationCoordinator>,
    id: String,
) -> Result<LibraryTree, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, move |context| {
            usecases::library::restore_trash(context, id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn move_library_nodes(
    coordinator: State<'_, OperationCoordinator>,
    request: MoveLibraryNodesRequest,
) -> Result<LibraryTree, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Library, move |context| {
            usecases::library::move_nodes(context, request)
        })
        .await
        .map_err(String::from)
}
