use tauri::State;

use crate::{
    backup::{BackupState, BackupTaskKind},
    cleanup,
};

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
        state.run_exclusive(None, BackupTaskKind::UserOperation, || {
            app_state.with_ready_repository(|root| cleanup::run(root, &ids))
        })
    })
    .await
    .map_err(|error| format!("文件清理重试任务异常结束：{error}"))?
}

/// 扫描仓库内未被引用的历史文件（崩溃孤儿等），只报告、不删除。
///
/// 与前台长命令同口径：持共享运行锁与仓库操作锁，可经统一取消入口取消；扫描
/// 入口在仓库哨兵锁落地前只经 GUI 进程内暴露，不做无头子命令。
#[tauri::command]
pub(crate) async fn scan_repository_unreferenced(
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<Vec<cleanup::ScanCandidate>, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, BackupTaskKind::UserOperation, || {
            app_state.with_ready_repository(|root| {
                state.report_progress("unreferenced-scan", "正在扫描未引用文件", 0, 0);
                cleanup::scan_unreferenced(
                    root,
                    || state.cancelled(),
                    |current, total| {
                        state.report_progress(
                            "unreferenced-scan",
                            "正在扫描未引用文件",
                            current,
                            total,
                        )
                    },
                )
            })
        })
    })
    .await
    .map_err(|error| format!("未引用文件扫描任务异常结束：{error}"))?
}

/// 把用户在设置页确认的扫描候选批量入队并单遍重放删除。
#[tauri::command]
pub(crate) async fn cleanup_repository_unreferenced(
    paths: Vec<String>,
    app_state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
) -> Result<cleanup::CleanupReport, String> {
    let app_state = app_state.inner().clone();
    let state = backup_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.run_exclusive(None, BackupTaskKind::UserOperation, || {
            app_state.with_ready_repository(|root| cleanup::cleanup_unreferenced(root, &paths))
        })
    })
    .await
    .map_err(|error| format!("未引用文件清理任务异常结束：{error}"))?
}
