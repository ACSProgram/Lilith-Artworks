use tauri::State;

use crate::{app::command_runtime::OperationDomain, cleanup};

use super::{usecases, OperationCoordinator};

#[tauri::command]
pub(crate) async fn retry_pending_file_cleanup(
    ids: Vec<String>,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<cleanup::CleanupReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Cleanup, move |context| {
            usecases::cleanup::retry_pending(context, &ids)
        })
        .await
        .map_err(String::from)
}

/// 列出待清理队列（路径、原因、上次失败信息），供设置页展示与重试。
///
/// 只读：走共享读租约、不取运行锁，因此在备份、恢复等长任务运行期间也能返回；
/// 它不执行删除，也不写 `last_attempt_ms` / `last_error`。
#[tauri::command]
pub(crate) async fn list_pending_file_cleanup(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Vec<cleanup::PendingCleanupEntry>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Cleanup, |context| {
            usecases::cleanup::list_pending(context)
        })
        .await
        .map_err(String::from)
}

/// 扫描仓库内未被引用的历史文件（崩溃孤儿等），只报告、不删除。
///
/// 与前台长命令同口径：持共享运行锁与仓库操作锁，可经统一取消入口取消；扫描
/// 入口在仓库哨兵锁落地前只经 GUI 进程内暴露，不做无头子命令。
#[tauri::command]
pub(crate) async fn scan_repository_unreferenced(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Vec<cleanup::ScanCandidate>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Cleanup, |context| {
            usecases::cleanup::scan_unreferenced(context)
        })
        .await
        .map_err(String::from)
}

/// 把用户在设置页确认的扫描候选批量入队并单遍重放删除。
#[tauri::command]
pub(crate) async fn cleanup_repository_unreferenced(
    paths: Vec<String>,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<cleanup::CleanupReport, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Cleanup, move |context| {
            usecases::cleanup::cleanup_unreferenced(context, &paths)
        })
        .await
        .map_err(String::from)
}
