use tauri::State;

use crate::{backup::BackupState, cleanup};

use super::AppState;

#[tauri::command]
pub(crate) async fn retry_pending_file_cleanup(
    ids: Vec<String>,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<cleanup::CleanupReport, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, || {
            app_state.with_ready_repository(|root| cleanup::run(root, &ids))
        })
    })
    .await
    .map_err(|error| format!("文件清理重试任务异常结束：{error}"))?
}
