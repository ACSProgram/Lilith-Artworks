use tauri::State;

use crate::{app::command_runtime::OperationDomain, storage_overview};

use super::{usecases, OperationCoordinator};

/// 仓储空间的数据库记录口径。只读：走共享读租约、不取运行锁，也不写任何状态。
#[tauri::command]
pub(crate) async fn get_repository_storage_overview(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<storage_overview::RepositoryStorageOverview, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Maintenance, |context| {
            usecases::storage::overview(context)
        })
        .await
        .map_err(String::from)
}

/// 仓库目录磁盘实际占用。按需触发的可取消长任务：取前台运行锁与仓库操作锁
/// 保证扫描期间本进程不写入，但不持有跨进程 owner lock。
#[tauri::command]
pub(crate) async fn scan_repository_storage_disk(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<storage_overview::RepositoryStorageDiskScan, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Maintenance, |context| {
            usecases::storage::scan_disk(context)
        })
        .await
        .map_err(String::from)
}
