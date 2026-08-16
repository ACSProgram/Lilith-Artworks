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

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryScrubReport {
    history_nodes: u64,
    final_artifacts: u64,
    certification_records: u64,
}

#[tauri::command]
pub(crate) async fn scrub_repository_integrity(
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<RepositoryScrubReport, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, || {
            app_state.with_ready_repository(|root| {
                state.report_progress("repository-scrub", "正在检查历史文件", 0, 0);
                let history_nodes = backup::scrub_history(
                    root,
                    || state.cancelled(),
                    |current, total| {
                        state.report_progress(
                            "repository-scrub",
                            "正在检查历史文件",
                            current,
                            total,
                        )
                    },
                )?;
                state.report_progress("repository-scrub", "正在检查发布文件", 0, 0);
                let controlled = authenticity::scrub_controlled_files(
                    root,
                    || state.cancelled(),
                    |current, total| {
                        state.report_progress(
                            "repository-scrub",
                            "正在检查发布文件",
                            current,
                            total,
                        )
                    },
                )?;
                Ok(RepositoryScrubReport {
                    history_nodes,
                    final_artifacts: controlled.0,
                    certification_records: controlled.1,
                })
            })
        })
    })
    .await
    .map_err(|error| format!("仓库完整性检查异常结束：{error}"))?
}

#[tauri::command]
pub(crate) fn acknowledge_backup_disable_notices(
    artwork_ids: Vec<String>,
    app_state: State<'_, AppState>,
) -> Result<(), String> {
    app_state.with_ready_repository(|root| {
        history::acknowledge_backup_disable_notices(root, &artwork_ids)
    })
}

#[tauri::command]
pub(crate) fn create_library_artwork(
    request: library::CreateArtworkRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<library::LibraryTree, String> {
    let tree = app_state.with_ready_repository(|root| {
        library::create_artwork_and_list(
            root,
            request.parent_id.as_deref(),
            &request.title,
            &request.branch_title,
            Path::new(&request.source_path),
        )
    })?;
    backup_state.wake_scheduler();
    Ok(tree)
}

#[tauri::command]
pub(crate) async fn permanently_delete_library_trash(
    ids: Vec<String>,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<cleanup::CleanupReport, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = state.run_exclusive(None, || {
            app_state.with_ready_repository(|root| {
                let cleanup_ids = library::permanently_delete_trash(root, &ids)?;
                cleanup::run(root, &cleanup_ids)
            })
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
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = state.run_exclusive(None, || {
            app_state.with_ready_repository(|root| {
                let cleanup_ids = library::empty_trash(root)?;
                cleanup::run(root, &cleanup_ids)
            })
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
    let result = backup_state.run_exclusive(None, || {
        app_state.with_ready_repository(|root| {
            backup::ensure_checkpoint(root, &request.from_history_id)?;
            history::create_branch(
                root,
                &request.artwork_id,
                &request.from_history_id,
                &request.title,
                Path::new(&request.source_path),
            )?;
            history::list(root, &request.artwork_id)
        })
    })?;
    backup_state.wake_scheduler();
    Ok(result)
}

#[tauri::command]
pub(crate) fn update_artwork_branch(
    request: history::UpdateBranchBackupRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<history::ArtworkHistory, String> {
    let result = backup_state.run_exclusive(None, || {
        app_state.with_ready_repository(|root| {
            history::update_branch(
                root,
                &request.branch_id,
                &request.title,
                request.expected_backup_enabled,
                request.backup_enabled,
                request.backup_interval_minutes,
            )?;
            let artwork_id = history::load_branch(root, &request.branch_id)?.artwork_id;
            history::list(root, &artwork_id)
        })
    })?;
    backup_state.wake_scheduler();
    Ok(result)
}

#[tauri::command]
pub(crate) async fn delete_artwork_branch(
    branch_id: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<history::ArtworkHistory, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, || {
            app_state.with_ready_repository(|root| {
                state.report_progress("delete-branch", "正在删除分支历史", 0, 1);
                let deletion = history::delete_branch(root, &branch_id)?;
                let artwork_id = deletion.artwork_id.clone();
                for relative in deletion.storage_paths {
                    if !history::storage_path_referenced(root, &relative)? {
                        let _ = std::fs::remove_file(storage::resolve_path(root, &relative)?);
                    }
                }
                state.report_progress("delete-branch", "分支删除完成", 1, 1);
                history::list(root, &artwork_id)
            })
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
    authenticity::ensure_dialog_authorized(
        &window,
        Path::new(request.artifact_path.trim()),
        "最终成品",
    )
    .map_err(|error| error.to_string())?;
    let state = backup_state.inner().clone();
    let app_state = app_state.inner().clone();
    let models_ready = authenticity_state.model_files_ready();
    let model_info = authenticity_state.model_info();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(Some(&request.branch_id), || {
            app_state.with_ready_repository(|root| {
                let (_, history_id) = authenticity::branch_head(root, &request.branch_id)?;
                state.report_progress("publish-lock", "正在固化发布检查点", 0, 2);
                backup::ensure_checkpoint(root, &history_id)?;
                state.report_progress("publish-lock", "正在保存最终成品", 1, 2);
                authenticity::store_final_artifact(
                    root,
                    &request.branch_id,
                    &history_id,
                    &request.artifact_path,
                )?;
                state.report_progress("publish-lock", "分支已进入发布状态", 2, 2);
                authenticity::get_publication(root, &request.branch_id, models_ready, model_info)
            })
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
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(Some(&branch_id), || {
            app_state.with_ready_repository(|root| {
                let cleanup_ids = authenticity::remove_artifact(root, &branch_id)?;
                cleanup::run(root, &cleanup_ids)
            })
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
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let branch_id = request.branch_id.clone();
        backup
            .run_exclusive(Some(&branch_id), || {
                app_state.with_ready_repository(|root| {
                    authenticity::publish_artifact(root, &authenticity, request)
                        .map_err(|error| error.to_string())
                })
            })
            .map_err(AuthenticityError::Task)
    })
    .await
    .map_err(|error| AuthenticityError::Task(error.to_string()))?
}
