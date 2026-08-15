use std::path::Path;

use tauri::State;

use crate::{
    app::AppState,
    backup::{self, BackupState},
    library,
};

use super::{
    ArtworkHistory, ForkBranchRequest, RenameHistoryNodeRequest, UpdateBranchBackupRequest,
};

fn root(state: &AppState) -> Result<std::path::PathBuf, String> {
    let root = state.repository_path()?.ok_or("尚未配置作品仓库")?;
    library::open_existing(&root)?;
    Ok(root)
}

#[tauri::command]
pub(crate) fn get_artwork_history(
    artwork_id: String,
    state: State<'_, AppState>,
) -> Result<ArtworkHistory, String> {
    super::list(&root(state.inner())?, &artwork_id)
}

#[tauri::command]
pub(crate) fn fork_artwork_branch(
    request: ForkBranchRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<ArtworkHistory, String> {
    let root = root(app_state.inner())?;
    backup_state.run_exclusive(None, || {
        backup::ensure_checkpoint(&root, &request.from_history_id)?;
        super::create_branch(
            &root,
            &request.artwork_id,
            &request.from_history_id,
            &request.title,
            Path::new(&request.source_path),
        )?;
        Ok(())
    })?;
    backup_state.wake_scheduler();
    super::list(&root, &request.artwork_id)
}

#[tauri::command]
pub(crate) fn update_artwork_branch(
    request: UpdateBranchBackupRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<ArtworkHistory, String> {
    let root = root(app_state.inner())?;
    super::update_branch(
        &root,
        &request.branch_id,
        &request.title,
        request.backup_enabled,
        request.backup_interval_minutes,
    )?;
    backup_state.wake_scheduler();
    let branch = super::load_branch(&root, &request.branch_id)?;
    super::list(&root, &branch.artwork_id)
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

#[tauri::command]
pub(crate) async fn delete_artwork_branch(
    branch_id: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<ArtworkHistory, String> {
    let root = root(app_state.inner())?;
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, || {
            state.report_progress("delete-branch", "正在删除分支历史", 0, 1);
            let deletion = super::delete_branch(&root, &branch_id)?;
            let artwork_id = deletion.artwork_id.clone();
            for relative in deletion.storage_paths {
                if !super::storage_path_referenced(&root, &relative)? {
                    let _ = std::fs::remove_file(crate::storage::resolve_path(&root, &relative)?);
                }
            }
            state.report_progress("delete-branch", "分支删除完成", 1, 1);
            super::list(&root, &artwork_id)
        })
    })
    .await
    .map_err(|error| format!("分支删除任务异常结束：{error}"))?
}
