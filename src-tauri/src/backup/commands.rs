use tauri::State;

use crate::{app::AppState, cleanup, history};

use super::{
    restore, worker, BackupCommitResult, BackupNowRequest, BackupRuntimeStatus, BackupState,
    BackupTaskKind,
};

#[tauri::command]
pub(crate) async fn run_branch_backup(
    request: BackupNowRequest,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<BackupCommitResult, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 手动提交优先：先登记待处理请求，让调度器延后该分支的自动备份；
        // 若同分支的后台任务正在运行，请求其让位，让手动提交先取得运行锁，
        // 避免自动任务抢先记录同一内容导致手动备注丢失。
        // `cancel_background` 只作用于后台任务，不会误取消其它用户操作。
        state.begin_manual_request(&request.branch_id);
        if let Ok(status) = state.status() {
            if status.active_branch_id.as_deref() == Some(request.branch_id.as_str()) {
                let _ = state.cancel_background();
            }
        }
        let result = state.run_logged(
            "manual backup",
            &format!(
                "branch_id={}, note_chars={}",
                request.branch_id,
                request.note.chars().count()
            ),
            Some(&request.branch_id),
            BackupTaskKind::UserOperation,
            || {
                app_state.with_ready_repository(|root| {
                    let result = worker::run_backup(
                        root,
                        &request.branch_id,
                        &request.note,
                        "manual",
                        || state.cancelled(),
                    );
                    if let Err(error) = result.as_ref() {
                        history::mark_error(root, &request.branch_id, &error.to_string());
                    }
                    result.map_err(|error| error.to_string())
                })
            },
        );
        state.end_manual_request(&request.branch_id);
        state.wake_scheduler();
        result
    })
    .await
    .map_err(|error| format!("分支提交任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn restore_history_node(
    history_id: String,
    output_path: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<String, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_logged_foreground(
            "restore",
            &format!("history_id={history_id}, output={output_path}"),
            None,
            || {
                app_state.with_ready_repository(|root| {
                    restore::restore(
                        root,
                        &history_id,
                        &output_path,
                        || state.cancelled(),
                        |label, current, total| {
                            state.report_progress("restore", label, current, total)
                        },
                    )
                })
            },
        )
    })
    .await
    .map_err(|error| format!("历史恢复任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn compact_history_node(
    history_id: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<(), String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_logged_foreground("compact", &format!("history_id={history_id}"), None, || {
            app_state.with_ready_repository(|root| {
                restore::compact_node(
                    root,
                    &history_id,
                    || state.cancelled(),
                    |label, current, total| state.report_progress("compact", label, current, total),
                )
            })
        })
    })
    .await
    .map_err(|error| format!("历史精简任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn delete_history_subtree(
    history_id: String,
    branch_id: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<String, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_logged_foreground(
            "history delete",
            &format!("history_id={history_id}, branch_id={branch_id}"),
            None,
            || {
                app_state.with_ready_repository(|root| {
                    let target = history::load_node(root, &history_id)?;
                    history::validate_subtree_deletion(root, &history_id, &branch_id)?;
                    if let Some(parent_id) = target.parent_id.as_deref() {
                        state.report_progress("delete", "正在固化保留历史", 0, 1);
                        restore::ensure_checkpoint_with_progress(
                            root,
                            parent_id,
                            || state.cancelled(),
                            |label, current, total| {
                                state.report_progress("delete", label, current, total)
                            },
                        )?;
                        state.report_progress("delete", "正在删除历史节点", 1, 1);
                    }
                    let deletion = history::delete_subtree(root, &history_id, &branch_id)?;
                    // 已无引用的历史文件已在删除事务内入队，提交成功后单遍重放；
                    // 失败只留队列可重试，不改变删除节点的成功语义。
                    cleanup::replay(root, &deletion.cleanup_ids);
                    Ok(deletion.artwork_id)
                })
            },
        )
    })
    .await
    .map_err(|error| format!("历史节点删除任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) async fn set_history_checkpoint(
    history_id: String,
    enabled: bool,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<(), String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_logged_foreground(
            "checkpoint",
            &format!("history_id={history_id}, enabled={enabled}"),
            None,
            || {
                app_state.with_ready_repository(|root| {
                    if enabled {
                        restore::ensure_checkpoint_with_progress(
                            root,
                            &history_id,
                            || state.cancelled(),
                            |label, current, total| {
                                state.report_progress("checkpoint", label, current, total)
                            },
                        )
                    } else {
                        // 取消检查点释放的 snapshot 已在事务内入队，提交成功后单遍重放。
                        if let Some(cleanup_ids) = history::unmark_checkpoint(root, &history_id)? {
                            state.report_progress(
                                "checkpoint",
                                "正在释放检查点并恢复增量统计",
                                0,
                                1,
                            );
                            cleanup::replay(root, &cleanup_ids);
                            state.report_progress("checkpoint", "检查点已取消", 1, 1);
                        }
                        Ok(())
                    }
                })
            },
        )
    })
    .await
    .map_err(|error| format!("检查点任务异常结束：{error}"))?
}

/// 清除分支的链路校验失败摘要，使其重新进入空闲校验队列，并唤醒调度器。
///
/// 与调度器"失败后退出队列、head 变化或手动重查才重新入队"的语义一致：这里只清空
/// `verify_error`，真正的校验仍由调度器在空闲时执行，不占用前台锁。
#[tauri::command]
pub(crate) async fn reverify_branch_history(
    branch_id: String,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<history::ArtworkHistory, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let artwork_id = app_state.with_repository_read(|root| {
            history::load_branch(root, &branch_id).map(|b| b.artwork_id)
        })?;
        app_state.with_ready_repository(|root| history::clear_verify_error(root, &branch_id))?;
        state.wake_scheduler();
        app_state.with_repository_read(|root| history::list(root, &artwork_id))
    })
    .await
    .map_err(|error| format!("重新校验任务异常结束：{error}"))?
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
