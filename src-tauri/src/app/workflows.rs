use std::path::Path;

use tauri::State;

use crate::{
    app::AppState,
    authenticity::{
        self, AuthenticityError, AuthenticityState, BranchPublication, EnterPublicationRequest,
        PublishBranchRequest, PublishResult,
    },
    backup::{self, BackupState},
    cleanup, history, library, storage,
};

fn root(state: &AppState) -> Result<std::path::PathBuf, String> {
    state.ready_repository_path()
}

#[tauri::command]
pub(crate) fn create_library_artwork(
    request: library::CreateArtworkRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<library::LibraryTree, String> {
    let root = root(app_state.inner())?;
    let tree = library::create_artwork_and_list(
        &root,
        request.parent_id.as_deref(),
        &request.title,
        &request.branch_title,
        Path::new(&request.source_path),
    )?;
    backup_state.wake_scheduler();
    Ok(tree)
}

#[tauri::command]
pub(crate) async fn permanently_delete_library_trash(
    ids: Vec<String>,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<cleanup::CleanupReport, String> {
    let root = root(app_state.inner())?;
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = state.run_exclusive(None, || {
            let cleanup_ids = library::permanently_delete_trash(&root, &ids)?;
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
    let root = root(app_state.inner())?;
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = state.run_exclusive(None, || {
            let cleanup_ids = library::empty_trash(&root)?;
            cleanup::run(&root, &cleanup_ids)
        })?;
        state.wake_scheduler();
        Ok(report)
    })
    .await
    .map_err(|error| format!("清空回收站任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) fn fork_artwork_branch(
    request: history::ForkBranchRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<history::ArtworkHistory, String> {
    let root = root(app_state.inner())?;
    backup_state.run_exclusive(None, || {
        backup::ensure_checkpoint(&root, &request.from_history_id)?;
        history::create_branch(
            &root,
            &request.artwork_id,
            &request.from_history_id,
            &request.title,
            Path::new(&request.source_path),
        )?;
        Ok(())
    })?;
    backup_state.wake_scheduler();
    history::list(&root, &request.artwork_id)
}

#[tauri::command]
pub(crate) fn update_artwork_branch(
    request: history::UpdateBranchBackupRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<history::ArtworkHistory, String> {
    let root = root(app_state.inner())?;
    history::update_branch(
        &root,
        &request.branch_id,
        &request.title,
        request.backup_enabled,
        request.backup_interval_minutes,
    )?;
    backup_state.wake_scheduler();
    let branch = history::load_branch(&root, &request.branch_id)?;
    history::list(&root, &branch.artwork_id)
}

#[tauri::command]
pub(crate) async fn delete_artwork_branch(
    branch_id: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<history::ArtworkHistory, String> {
    let root = root(app_state.inner())?;
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, || {
            state.report_progress("delete-branch", "正在删除分支历史", 0, 1);
            let deletion = history::delete_branch(&root, &branch_id)?;
            let artwork_id = deletion.artwork_id.clone();
            for relative in deletion.storage_paths {
                if !history::storage_path_referenced(&root, &relative)? {
                    let _ = std::fs::remove_file(storage::resolve_path(&root, &relative)?);
                }
            }
            state.report_progress("delete-branch", "分支删除完成", 1, 1);
            history::list(&root, &artwork_id)
        })
    })
    .await
    .map_err(|error| format!("分支删除任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn enter_branch_publication(
    request: EnterPublicationRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    authenticity_state: State<'_, AuthenticityState>,
    window: tauri::WebviewWindow,
) -> Result<BranchPublication, String> {
    let root = root(app_state.inner())?;
    authenticity::ensure_dialog_authorized(
        &window,
        Path::new(request.artifact_path.trim()),
        "最终成品",
    )
    .map_err(|error| error.to_string())?;
    let state = backup_state.inner().clone();
    let models_ready = authenticity_state.model_files_ready();
    let model_info = authenticity_state.model_info();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(Some(&request.branch_id), || {
            let (_, history_id) = authenticity::branch_head(&root, &request.branch_id)?;
            state.report_progress("publish-lock", "正在固化发布检查点", 0, 2);
            backup::ensure_checkpoint(&root, &history_id)?;
            state.report_progress("publish-lock", "正在保存最终成品", 1, 2);
            authenticity::store_final_artifact(
                &root,
                &request.branch_id,
                &history_id,
                &request.artifact_path,
            )?;
            state.report_progress("publish-lock", "分支已进入发布状态", 2, 2);
            authenticity::get_publication(&root, &request.branch_id, models_ready, model_info)
        })
    })
    .await
    .map_err(|error| format!("进入发布状态的任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn cancel_branch_publication(
    branch_id: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<cleanup::CleanupReport, String> {
    let root = root(app_state.inner())?;
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(Some(&branch_id), || {
            let cleanup_ids = authenticity::remove_artifact(&root, &branch_id)?;
            cleanup::run(&root, &cleanup_ids)
        })
    })
    .await
    .map_err(|error| format!("取消发布任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn publish_branch_artifact(
    request: PublishBranchRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    authenticity_state: State<'_, AuthenticityState>,
    window: tauri::WebviewWindow,
) -> Result<PublishResult, AuthenticityError> {
    let root = root(app_state.inner()).map_err(AuthenticityError::Task)?;
    authenticity::ensure_dialog_authorized(
        &window,
        Path::new(request.output_path.trim()),
        "发布输出路径",
    )?;
    authenticity::ensure_dialog_authorized(
        &window,
        Path::new(request.config.certificate_path.trim()),
        "证书链",
    )?;
    let authenticity = authenticity_state.inner().clone();
    let backup = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let branch_id = request.branch_id.clone();
        backup
            .run_exclusive(Some(&branch_id), || {
                authenticity::publish_artifact(&root, &authenticity, request)
                    .map_err(|error| error.to_string())
            })
            .map_err(AuthenticityError::Task)
    })
    .await
    .map_err(|error| AuthenticityError::Task(error.to_string()))?
}
