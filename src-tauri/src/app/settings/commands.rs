//! 设置相关的 Tauri 命令与仓库准备。
//!
//! 命令只做参数/scope 校验与调用编排，业务写入经 `usecases::settings`；仓库初始化
//! 仍在设置入口内完成，不把仓库初始化逻辑移入 Library。窗口几何、设置文件格式与
//! 进程状态分别在 `window`、`model`、`state`。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, State};

use crate::app::{command_runtime::OperationDomain, usecases, OperationCoordinator};
use crate::authenticity::preview_session_id;
use crate::backup::BackupState;
use crate::platform::{
    path::ensure_not_device_path,
    shell::{open_directory, open_existing_directory, reveal_in_file_manager},
};
use crate::{cleanup, library};

use super::model::AppSettings;
use super::state::{AppState, SettingsSnapshot};

#[tauri::command]
pub(crate) async fn get_app_settings(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<SettingsSnapshot, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Settings, |context| {
            usecases::settings::snapshot_for_command(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) fn open_log_directory(state: State<'_, AppState>) -> Result<(), String> {
    open_directory(state.log_directory(), "日志")
}

#[tauri::command]
pub(crate) fn open_settings_directory(state: State<'_, AppState>) -> Result<(), String> {
    let directory = state.settings_path().parent().ok_or("设置文件路径无效")?;
    open_directory(directory, "设置")
}

/// 在系统文件管理器中打开给定文件所在的文件夹（Windows 上会选中该文件）。
#[tauri::command]
pub(crate) fn reveal_path_in_folder(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("文件路径为空".into());
    }
    // 用户在场的手动操作：网络位置照常打开（NAS 是合法用法），但设备命名空间
    // 一律拒绝——那是设备对象而不是文件，交给资源管理器打开没有意义。
    ensure_not_device_path(trimmed, "文件路径")?;
    let path = Path::new(trimmed);
    if !path.is_file() {
        return Err("文件不存在或已被移动".into());
    }
    reveal_in_file_manager(path)
}

#[tauri::command]
pub(crate) fn open_legal_directory(app: AppHandle) -> Result<(), String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| format!("无法定位应用资源目录：{error}"))?;
    let candidates = [
        resource_dir.join("licenses"),
        resource_dir.join("resources").join("licenses"),
    ];
    let directory = candidates
        .iter()
        .find(|path| path.join("THIRD_PARTY_LICENSES.html").is_file())
        .ok_or("随包法律材料缺失；请重新安装应用")?;
    open_existing_directory(directory, "法律材料")
}

#[tauri::command]
pub(crate) async fn save_app_settings(
    app: AppHandle,
    coordinator: State<'_, OperationCoordinator>,
    settings: AppSettings,
) -> Result<SettingsSnapshot, String> {
    let coordinator = coordinator.inner().clone();
    let next = coordinator
        .spawn(OperationDomain::Settings, move |context| {
            usecases::settings::save(context, settings)
        })
        .await
        .map_err(String::from)?;
    crate::refresh_tray_backup_menu(&app)?;
    Ok(next)
}

pub(crate) fn prepare_repository(
    current_repository: Option<&Path>,
    requested: &str,
) -> Result<Option<PathBuf>, String> {
    if requested.is_empty() {
        return Ok(None);
    }
    let root = Path::new(requested);
    if current_repository == Some(root) {
        library::open_existing(root)?;
    } else {
        library::initialize(root)?;
    }
    sweep_stale_repository_temp(root);
    Ok(Some(root.to_path_buf()))
}

/// 仓库就绪后清扫超期的仓库 `temp/`（认证预览会话目录与 `.tmp*` 暂存残留）。
/// 清扫只碰不被引用的临时条目，失败只记日志，绝不阻断仓库打开。
fn sweep_stale_repository_temp(root: &Path) {
    let report = cleanup::sweep_repository_temp(root, preview_session_id());
    if report.reclaimed > 0 || report.failed > 0 {
        log::info!(
            "仓库打开时清扫临时目录：回收 {} 项，失败 {} 项",
            report.reclaimed,
            report.failed
        );
    }
}

pub(crate) fn set_automatic_backups_paused(app: &AppHandle, paused: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    state
        .settings()
        .update(None, |settings| {
            settings.pause_automatic_backups = paused;
            Ok(())
        })
        .map_err(String::from)?;
    let backup = app.state::<BackupState>();
    backup.set_automatic_scheduling(!paused);
    backup.wake_scheduler();
    crate::refresh_tray_backup_menu(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn repository_save_only_initializes_a_new_selection() {
        let directory = tempfile::tempdir().unwrap();
        let missing_existing = directory.path().join("missing-existing");
        fs::create_dir(&missing_existing).unwrap();

        assert!(
            prepare_repository(Some(&missing_existing), &missing_existing.to_string_lossy())
                .is_err()
        );
        assert!(!crate::storage::database_path(&missing_existing).exists());

        let new_repository = directory.path().join("new-repository");
        prepare_repository(None, &new_repository.to_string_lossy()).unwrap();
        assert!(crate::storage::database_path(&new_repository).is_file());
    }
}
