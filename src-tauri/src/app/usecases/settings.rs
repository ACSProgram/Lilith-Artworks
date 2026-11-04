use std::path::{Path, PathBuf};

use crate::app::{
    command_runtime::{CommandContext, CommandError},
    settings::{
        check_repository_target, prepare_repository, repository_counts, snapshot,
        snapshot_without_repository, validate_settings, AppSettings, SettingsSnapshot,
        CURRENT_SETTINGS_VERSION,
    },
};
use crate::library::{self, RepositoryPathState};

/// 保存仓库路径前的只读探测。只报告五种目录状态，不创建目录、不初始化数据库。
pub(crate) fn inspect_repository_path(path: &str) -> Result<RepositoryPathState, CommandError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(CommandError::invalid_input("作品仓库路径为空"));
    }
    library::inspect_path(Path::new(trimmed)).map_err(CommandError::from)
}

/// 设置保存与仓库切换的唯一业务入口。设置页、托盘暂停开关与窗口几何捕获都写同一
/// 份 `SettingsService` 快照；这里只负责命令语义（校验、切仓库、刷新调度器），
/// revision/CAS 由 service 负责。
pub(crate) fn save(
    context: &CommandContext,
    settings: AppSettings,
    allow_create: bool,
) -> Result<SettingsSnapshot, CommandError> {
    validate_settings(&settings).map_err(CommandError::from)?;
    if settings.version != CURRENT_SETTINGS_VERSION {
        return Err(CommandError::invalid_input("设置版本不受支持"));
    }
    let app_state = context.app_state().clone();
    let paused = settings.pause_automatic_backups;
    let requested_repository = settings.repository_path.trim().to_owned();
    let expected_revision = settings.revision;
    let repository_changed = app_state
        .repository_path()
        .map_err(CommandError::from)?
        .map(|path| path.to_string_lossy().into_owned())
        != Some(requested_repository.clone());
    // 切换/首次配置时先保证目标目录存在再取 owner lock；否则锁库无处落盘。
    // 取锁会写入目标根目录，所以先做只读预检：被拒绝的保存不在用户目录里留文件。
    let target_repository = if repository_changed && !requested_repository.is_empty() {
        let path = PathBuf::from(&requested_repository);
        check_repository_target(&path, allow_create).map_err(CommandError::from)?;
        if !path.exists() {
            std::fs::create_dir_all(&path)
                .map_err(|error| CommandError::from(format!("无法创建作品仓库目录：{error}")))?;
        }
        Some(path)
    } else {
        None
    };

    let next =
        context.run_repository_switch(target_repository.as_deref(), repository_changed, || {
            let current_repository = app_state.repository_path().map_err(CommandError::from)?;
            let prepared_repository = prepare_repository(
                current_repository.as_deref(),
                &requested_repository,
                allow_create,
            )
            .map_err(CommandError::from)?;
            app_state
                .settings()
                .update(Some(expected_revision), |current| {
                    *current = settings.clone();
                    Ok(())
                })?;
            app_state.clear_warning().map_err(CommandError::from)?;
            app_state
                .set_validated_repository(prepared_repository)
                .map_err(CommandError::from)?;
            log::info!(
                "settings saved: repository_changed={repository_changed}, repository_path={}",
                app_state
                    .repository_path()
                    .map_err(CommandError::from)?
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            );
            snapshot(&app_state).map_err(CommandError::from)
        })?;

    context.backup_state().set_automatic_scheduling(!paused);
    context.backup_state().wake_scheduler();
    Ok(next)
}

/// 设置快照读取：保持 0.2 的宽松语义——仓库不可读只让自动备份计数为空，
/// 不阻断设置读取。仓库派生的计数在共享读租约内读取，避免与仓库切换并发。
pub(crate) fn snapshot_for_command(
    context: &CommandContext,
) -> Result<SettingsSnapshot, CommandError> {
    let mut snapshot =
        snapshot_without_repository(context.app_state()).map_err(CommandError::from)?;
    if let Ok((file_count, pending)) = context.run_read("settings_snapshot", |session| {
        Ok(repository_counts(session.root()))
    }) {
        snapshot.automatic_backup_file_count = file_count;
        snapshot.idle_integrity_pending_count = pending;
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{mpsc, Arc},
        thread,
        time::Duration,
    };

    use crate::{
        app::{
            command_runtime::{NoopEventSink, OperationDomain},
            AppState, OperationCoordinator,
        },
        backup::BackupState,
        library,
    };

    use super::*;

    #[test]
    fn saving_a_missing_repository_requires_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("new-repository");
        let state = AppState::new(
            AppSettings::default(),
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        let coordinator = OperationCoordinator::new(
            state.clone(),
            BackupState::default(),
            Arc::new(NoopEventSink),
        );
        let mut next = AppSettings::default();
        next.repository_path = repository.to_string_lossy().into_owned();

        let rejected = coordinator.execute(OperationDomain::Settings, |context| {
            save(context, next.clone(), false)
        });
        let error = match rejected {
            Err(error) => String::from(error),
            Ok(_) => panic!("missing repository must require confirmation"),
        };
        assert!(error.contains("确认新建仓库"), "{error}");
        assert!(!repository.exists());
        assert!(state.repository_path().unwrap().is_none());

        coordinator
            .execute(OperationDomain::Settings, |context| {
                save(context, next, true)
            })
            .unwrap();
        assert!(library::database_path(&repository).is_file());
    }

    #[test]
    fn rejected_repository_saves_leave_the_target_directory_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let empty = directory.path().join("empty");
        let occupied = directory.path().join("occupied");
        std::fs::create_dir(&empty).unwrap();
        std::fs::create_dir(&occupied).unwrap();
        std::fs::write(occupied.join("notes.txt"), b"not a repository").unwrap();

        let state = AppState::new(
            AppSettings::default(),
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        let coordinator = OperationCoordinator::new(
            state.clone(),
            BackupState::default(),
            Arc::new(NoopEventSink),
        );

        for target in [&empty, &occupied] {
            let mut next = AppSettings::default();
            next.repository_path = target.to_string_lossy().into_owned();
            let rejected = coordinator.execute(OperationDomain::Settings, |context| {
                save(context, next, false)
            });
            assert!(rejected.is_err(), "未确认新建时不得保存未就绪的目录");
            // owner lock 会在目标根目录写锁库：被拒绝的保存不能在用户目录里留下任何文件。
            let entries = std::fs::read_dir(target).unwrap().count();
            let expected = if target == &occupied { 1 } else { 0 };
            assert_eq!(entries, expected, "{}", target.display());
        }
        assert!(state.repository_path().unwrap().is_none());
    }

    #[test]
    fn repository_switch_waits_for_an_active_repository_lease() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        library::initialize(&first).unwrap();
        library::initialize(&second).unwrap();

        let mut settings = AppSettings::default();
        settings.repository_path = first.to_string_lossy().into_owned();
        let state = AppState::new(
            settings,
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        let coordinator = OperationCoordinator::new(
            state.clone(),
            BackupState::default(),
            Arc::new(NoopEventSink),
        );

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let reader = state.clone();
        let reader_thread = thread::spawn(move || {
            reader
                .with_repository_read(|_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let mut next = AppSettings::default();
        next.repository_path = second.to_string_lossy().into_owned();
        let switching = coordinator.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let switch_thread = thread::spawn(move || {
            let result = switching.execute(OperationDomain::Settings, |context| {
                save(context, next, false)
            });
            done_tx.send(result).unwrap();
        });

        assert!(
            done_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "仓库切换必须等待活动租约结束"
        );
        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        reader_thread.join().unwrap();
        switch_thread.join().unwrap();
        assert_eq!(
            state.repository_path().unwrap().as_deref(),
            Some(second.as_path())
        );
    }
}
