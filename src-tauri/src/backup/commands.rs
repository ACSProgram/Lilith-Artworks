use tauri::State;

use std::{fs, fs::File};

use tempfile::NamedTempFile;

use crate::{app::AppState, history, storage};

use super::{
    restore, worker, BackupCommitResult, BackupNowRequest, BackupRuntimeStatus, BackupState,
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
        let result = state.run_exclusive(Some(&request.branch_id), || {
            app_state.with_ready_repository(|root| {
                let result =
                    worker::run_backup(root, &request.branch_id, &request.note, "manual", || {
                        state.cancelled()
                    });
                if let Err(error) = result.as_ref() {
                    history::mark_error(root, &request.branch_id, error);
                }
                result
            })
        });
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
        state.run_exclusive(None, || {
            app_state.with_ready_repository(|root| {
                restore::restore(
                    root,
                    &history_id,
                    &output_path,
                    || state.cancelled(),
                    |label, current, total| state.report_progress("restore", label, current, total),
                )
            })
        })
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
        state.run_exclusive(None, || {
            app_state.with_ready_repository(|root| {
                let target = history::compaction_target(root, &history_id)?;
                let parent_snapshot = restore::materialize_snapshot(
                    root,
                    &target.parent_id,
                    || state.cancelled(),
                    |label, current, total| state.report_progress("compact", label, current, total),
                )?;
                let child_snapshot = restore::materialize_snapshot(
                    root,
                    &target.child_id,
                    || state.cancelled(),
                    |label, current, total| state.report_progress("compact", label, current, total),
                )?;
                let artwork_directory = history::artwork_directory(root, &target.artwork_id);
                let delta_directory = artwork_directory.join("deltas");
                fs::create_dir_all(&delta_directory)
                    .map_err(|error| format!("无法创建精简 delta 目录：{error}"))?;
                let delta_name = format!("{}-to-{}.lbd", target.child_id, target.parent_id);
                let delta_final = delta_directory.join(delta_name);
                let delta_relative = storage::relative_path(root, &delta_final)?;
                let mut delta_temp = NamedTempFile::new_in(artwork_directory.join("temp"))
                    .map_err(|error| format!("无法创建精简 delta：{error}"))?;
                let mut parent_file = File::open(parent_snapshot.path())
                    .map_err(|error| format!("无法读取精简父节点：{error}"))?;
                let parent_chunk = super::chunk_file::ChunkFile::open(&mut parent_file)
                    .map_err(|error| format!("无法解析精简父节点：{error}"))?;
                let mut child_file = File::open(child_snapshot.path())
                    .map_err(|error| format!("无法读取精简子节点：{error}"))?;
                let child_chunk = super::chunk_file::ChunkFile::open(&mut child_file)
                    .map_err(|error| format!("无法解析精简子节点：{error}"))?;
                child_chunk
                    .create_reverse_delta(&parent_chunk, &mut parent_file, delta_temp.as_file_mut())
                    .map_err(|error| format!("无法重建精简 delta：{error}"))?;
                delta_temp
                    .as_file()
                    .sync_all()
                    .map_err(|error| format!("无法同步精简 delta：{error}"))?;
                if state.cancelled() {
                    return Err("精简操作已取消".into());
                }
                let delta_size = delta_temp
                    .as_file()
                    .metadata()
                    .map_err(|error| format!("无法读取精简 delta 大小：{error}"))?
                    .len();
                delta_temp
                    .persist_noclobber(&delta_final)
                    .map_err(|error| format!("无法发布精简 delta：{}", error.error))?;
                state.report_progress("compact", "正在改接历史链", 1, 1);
                let old_paths =
                    match history::apply_compaction(root, &target, &delta_relative, delta_size) {
                        Ok(paths) => paths,
                        Err(error) => {
                            let _ = fs::remove_file(&delta_final);
                            return Err(error);
                        }
                    };
                for relative in old_paths {
                    if relative != delta_relative
                        && !history::storage_path_referenced(root, &relative)?
                    {
                        let path = storage::resolve_path(root, &relative)?;
                        let _ = fs::remove_file(path);
                    }
                }
                Ok(())
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
        state.run_exclusive(None, || {
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
                for relative in deletion.storage_paths {
                    if !history::storage_path_referenced(root, &relative)? {
                        let _ = fs::remove_file(storage::resolve_path(root, &relative)?);
                    }
                }
                Ok(deletion.artwork_id)
            })
        })
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
        state.run_exclusive(None, || {
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
                } else if let Some(relative) = history::unmark_checkpoint(root, &history_id)? {
                    state.report_progress("checkpoint", "正在释放检查点并恢复增量统计", 0, 1);
                    if !history::storage_path_referenced(root, &relative)? {
                        let _ = fs::remove_file(storage::resolve_path(root, &relative)?);
                    }
                    state.report_progress("checkpoint", "检查点已取消", 1, 1);
                    Ok(())
                } else {
                    Ok(())
                }
            })
        })
    })
    .await
    .map_err(|error| format!("检查点任务异常结束：{error}"))?
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
