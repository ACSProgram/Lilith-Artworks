use tauri::State;

use crate::app::{command_runtime::OperationDomain, usecases, OperationCoordinator};

use super::{BackupCommitResult, BackupNowRequest, BackupRuntimeStatus, BackupState};

#[tauri::command]
pub(crate) async fn run_branch_backup(
    request: BackupNowRequest,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<BackupCommitResult, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Backup, move |context| {
            usecases::backup::commit(context, &request.branch_id, &request.note, "manual", true)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn restore_history_node(
    history_id: String,
    output_path: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<String, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Backup, move |context| {
            usecases::backup::restore(context, &history_id, &output_path)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn compact_history_node(
    history_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<(), String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Backup, move |context| {
            usecases::backup::compact(context, &history_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn delete_history_subtree(
    history_id: String,
    branch_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<String, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Backup, move |context| {
            usecases::backup::delete_subtree(context, &history_id, &branch_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn set_history_checkpoint(
    history_id: String,
    enabled: bool,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<(), String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Backup, move |context| {
            usecases::backup::set_checkpoint(context, &history_id, enabled)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn reverify_branch_history(
    branch_id: String,
    coordinator: State<'_, OperationCoordinator>,
) -> Result<crate::history::ArtworkHistory, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Backup, move |context| {
            usecases::history::reverify_branch(context, &branch_id)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) async fn get_last_repository_backup_run(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<Option<crate::backup::RepositoryBackupRun>, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Backup, |context| {
            usecases::backup::last_repository_backup(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) fn get_backup_runtime_status(
    state: State<'_, BackupState>,
) -> Result<BackupRuntimeStatus, String> {
    state.status()
}

#[tauri::command]
pub(crate) fn cancel_backup_operation(state: State<'_, BackupState>) -> Result<bool, String> {
    state.request_cancel()
}
