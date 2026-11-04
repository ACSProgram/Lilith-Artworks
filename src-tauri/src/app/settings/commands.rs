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
use crate::library::RepositoryPathState;
use crate::platform::{
    path::ensure_not_device_path,
    shell::{open_directory, open_existing_directory, reveal_in_file_manager},
};
use crate::{cleanup, library};

use super::model::AppSettings;
use super::state::{AppState, SettingsSnapshot};

/// 保存仓库路径前的只读探测：`missing` / `not_directory` / `empty_directory` /
/// `repository` / `non_repository_directory`。不创建目录，也不初始化数据库。
#[tauri::command]
pub(crate) fn inspect_repository_path(path: String) -> Result<RepositoryPathState, String> {
    usecases::settings::inspect_repository_path(&path).map_err(String::from)
}

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
    allow_create: bool,
) -> Result<SettingsSnapshot, String> {
    let coordinator = coordinator.inner().clone();
    let next = coordinator
        .spawn(OperationDomain::Settings, move |context| {
            usecases::settings::save(context, settings, allow_create)
        })
        .await
        .map_err(String::from)?;
    crate::refresh_tray_backup_menu(&app)?;
    Ok(next)
}

/// 只读预检：这次保存能不能落到目标目录。
///
/// `allow_create` 为假时只接受已有仓库；为真时额外接受不存在的目录与空目录。取跨进程
/// owner lock 会往目标根目录写锁库，因此保存路径必须在取锁前先跑这一遍，否则被拒绝
/// 的保存会在用户选中的目录里留下 `.lilith-write-lock*`。
pub(crate) fn check_repository_target(root: &Path, allow_create: bool) -> Result<(), String> {
    match library::inspect_path(root)? {
        RepositoryPathState::Repository => Ok(()),
        RepositoryPathState::Missing if allow_create => Ok(()),
        RepositoryPathState::EmptyDirectory if allow_create => Ok(()),
        RepositoryPathState::Missing => {
            Err("所选作品仓库目录不存在；请确认新建仓库后再保存".into())
        }
        RepositoryPathState::EmptyDirectory => {
            Err("所选作品仓库目录为空；请确认新建仓库后再保存".into())
        }
        RepositoryPathState::NotDirectory => Err("所选作品仓库路径不是目录".into()),
        RepositoryPathState::NonRepositoryDirectory => {
            Err("所选目录非空，且不是 Lilith Artworks 作品仓库；请选择空目录或已有仓库".into())
        }
    }
}

pub(crate) fn prepare_repository(
    current_repository: Option<&Path>,
    requested: &str,
    allow_create: bool,
) -> Result<Option<PathBuf>, String> {
    if requested.is_empty() {
        return Ok(None);
    }
    let root = Path::new(requested);
    if current_repository == Some(root) {
        library::open_existing(root)?;
    } else if allow_create {
        // 已确认新建：只允许不存在的目录或空目录；`initialize` 仍会拒绝
        // 非空非仓库目录，并允许可识别的旧库走迁移。
        library::initialize(root)?;
    } else {
        // 未确认时只打开既有仓库，绝不初始化。这里重跑一遍预检，因为预检与取锁之间
        // 目录状态可能被外部改动。
        check_repository_target(root, false)?;
        library::open_existing(root)?;
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

        assert!(prepare_repository(
            Some(&missing_existing),
            &missing_existing.to_string_lossy(),
            false
        )
        .is_err());
        assert!(!crate::storage::database_path(&missing_existing).exists());

        let new_repository = directory.path().join("new-repository");
        prepare_repository(None, &new_repository.to_string_lossy(), true).unwrap();
        assert!(crate::storage::database_path(&new_repository).is_file());
    }

    #[test]
    fn repository_save_refuses_creation_until_confirmed() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        let error = prepare_repository(None, &missing.to_string_lossy(), false).unwrap_err();
        assert!(error.contains("确认新建仓库"), "{error}");
        assert!(!missing.exists());

        let empty = directory.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let error = prepare_repository(None, &empty.to_string_lossy(), false).unwrap_err();
        assert!(error.contains("确认新建仓库"), "{error}");
        assert!(!crate::storage::database_path(&empty).exists());

        prepare_repository(None, &empty.to_string_lossy(), true).unwrap();
        assert!(crate::storage::database_path(&empty).is_file());
    }

    #[test]
    fn repository_save_rejects_a_non_repository_directory_with_or_without_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let occupied = directory.path().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("notes.txt"), b"not a repository").unwrap();

        for allow_create in [false, true] {
            let error =
                prepare_repository(None, &occupied.to_string_lossy(), allow_create).unwrap_err();
            assert!(error.contains("Lilith Artworks"), "{error}");
            assert!(!crate::storage::database_path(&occupied).exists());
        }
    }

    #[test]
    fn repository_save_opens_an_existing_repository_without_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory.path().join("existing");
        library::initialize(&existing).unwrap();

        prepare_repository(None, &existing.to_string_lossy(), false).unwrap();
        assert!(crate::storage::database_path(&existing).is_file());
    }
}
