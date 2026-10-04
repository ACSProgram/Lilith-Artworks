use std::path::Path;

use tauri::State;

use crate::{
    app::AppState,
    authenticity::{
        self, AuthenticityError, AuthenticityState, BranchPublication, EnterPublicationRequest,
        PublishBranchRequest, PublishResult,
    },
    backup::{self, BackupState, BackupTaskKind},
    cleanup, history, library, pin_board,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryScrubReport {
    history_nodes: u64,
    final_artifacts: u64,
    certification_records: u64,
    /// 检查的画板图片记录数。
    pin_board_images: u64,
    /// 记录存在但 DDS 缺失。
    pin_board_missing_dds: u64,
    /// DDS 存在但校验失败（路径归属、头、尺寸、长度或解码）。
    pin_board_corrupt_dds: u64,
    /// 磁盘上存在但无记录的孤儿 DDS。
    pin_board_orphan_dds: u64,
}

#[tauri::command]
pub(crate) async fn scrub_repository_integrity(
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<RepositoryScrubReport, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_foreground(None, || {
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
                state.report_progress("repository-scrub", "正在检查画板图片", 0, 0);
                let board_dds = pin_board::scrub::scrub_board_dds(
                    root,
                    || state.cancelled(),
                    |current, total| {
                        state.report_progress(
                            "repository-scrub",
                            "正在检查画板图片",
                            current,
                            total,
                        )
                    },
                )?;
                Ok(RepositoryScrubReport {
                    history_nodes,
                    final_artifacts: controlled.0,
                    certification_records: controlled.1,
                    pin_board_images: board_dds.images,
                    pin_board_missing_dds: board_dds.missing,
                    pin_board_corrupt_dds: board_dds.corrupt,
                    pin_board_orphan_dds: board_dds.orphans,
                })
            })
        })
    })
    .await
    .map_err(|error| format!("仓库完整性检查异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn create_repository_backup(
    destination_parent: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    window: tauri::WebviewWindow,
) -> Result<backup::RepositoryBackupReport, String> {
    let destination_parent = std::path::PathBuf::from(destination_parent.trim());
    authenticity::ensure_dialog_authorized(&window, &destination_parent, "备份保存目录")
        .map_err(|error| error.to_string())?;
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_foreground(None, || {
            app_state.with_ready_repository(|root| {
                backup::create_repository_backup(
                    root,
                    &destination_parent,
                    || state.cancelled(),
                    |label, current, total| {
                        state.report_progress("repository-backup", label, current, total)
                    },
                )
            })
        })
    })
    .await
    .map_err(|error| format!("创建备份任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn acknowledge_backup_disable_notices(
    artwork_ids: Vec<String>,
    app_state: State<'_, AppState>,
) -> Result<(), String> {
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        app_state.with_ready_repository(|root| {
            history::acknowledge_backup_disable_notices(root, &artwork_ids)
        })
    })
    .await
    .map_err(|error| format!("备份告警确认任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn get_backup_disable_notice_target(
    app_state: State<'_, AppState>,
) -> Result<Option<history::BackupDisableNoticeTarget>, String> {
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        app_state.with_repository_read(history::next_backup_disable_notice_target)
    })
    .await
    .map_err(|error| format!("备份告警查询任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn create_library_artwork(
    request: library::CreateArtworkRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<library::LibraryTree, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let tree = app_state.with_ready_repository(|root| {
            library::create_artwork_and_list(
                root,
                request.parent_id.as_deref(),
                &request.title,
                &request.branch_title,
                Path::new(&request.source_path),
            )
        })?;
        state.wake_scheduler();
        Ok(tree)
    })
    .await
    .map_err(|error| format!("创建 Artwork 任务异常结束：{error}"))?
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
        let report = state.run_exclusive(None, BackupTaskKind::UserOperation, || {
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
        let report = state.run_exclusive(None, BackupTaskKind::UserOperation, || {
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
pub(crate) async fn fork_artwork_branch(
    request: history::ForkBranchRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<history::ArtworkHistory, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    let scheduler = state.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, BackupTaskKind::UserOperation, || {
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
        })
    })
    .await
    .map_err(|error| format!("创建分支任务异常结束：{error}"))?;
    scheduler.wake_scheduler();
    result
}

#[tauri::command]
pub(crate) async fn update_artwork_branch(
    request: history::UpdateBranchBackupRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<history::ArtworkHistory, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    let scheduler = state.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, BackupTaskKind::UserOperation, || {
            app_state.with_ready_repository(|root| {
                history::update_branch(
                    root,
                    &request.branch_id,
                    &request.title,
                    request.expected_backup_enabled,
                    request.backup_enabled,
                    request.backup_interval_minutes,
                    request.backup_quick_enabled,
                    request.source_path.as_deref(),
                )?;
                let artwork_id = history::load_branch(root, &request.branch_id)?.artwork_id;
                history::list(root, &artwork_id)
            })
        })
    })
    .await
    .map_err(|error| format!("分支设置保存任务异常结束：{error}"))?;
    scheduler.wake_scheduler();
    result
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
        state.run_exclusive(None, BackupTaskKind::UserOperation, || {
            app_state.with_ready_repository(|root| {
                state.report_progress("delete-branch", "正在删除分支历史", 0, 1);
                let deletion = history::delete_branch(root, &branch_id)?;
                let artwork_id = deletion.artwork_id.clone();
                // 已无引用的历史文件已在删除事务内入队，提交成功后单遍重放；
                // 删除失败只留队列可重试，不改变分支删除的成功语义。
                cleanup::replay(root, &deletion.cleanup_ids);
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
        state.run_foreground(Some(&request.branch_id), || {
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
        state.run_exclusive(Some(&branch_id), BackupTaskKind::UserOperation, || {
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
    let operation = authenticity_state.begin_operation("认证签名发布")?;
    let authenticity = authenticity_state.inner().clone();
    let backup = backup_state.inner().clone();
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let branch_id = request.branch_id.clone();
        backup
            .run_foreground(Some(&branch_id), || {
                app_state.with_ready_repository(|root| {
                    authenticity::publish_artifact(root, &authenticity, &operation, request)
                        .map_err(|error| error.to_string())
                })
            })
            .map_err(AuthenticityError::Task)
    })
    .await
    .map_err(|error| AuthenticityError::Task(error.to_string()))?
}
