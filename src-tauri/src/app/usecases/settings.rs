use crate::app::{
    command_runtime::{CommandContext, CommandError},
    settings::{
        prepare_repository, snapshot, validate_settings, write_json_atomic, AppSettings,
        SettingsSnapshot, CURRENT_SETTINGS_VERSION,
    },
};

/// 设置保存与仓库切换的唯一业务入口。托盘写入在阶段 2 收敛到同一 service；
/// 阶段 1 先把设置页入口纳入 coordinator，保持命令名、DTO 与用户文案不变。
pub(crate) fn save(
    context: &CommandContext,
    settings: AppSettings,
) -> Result<SettingsSnapshot, CommandError> {
    validate_settings(&settings).map_err(CommandError::from)?;
    if settings.version != CURRENT_SETTINGS_VERSION {
        return Err(CommandError::invalid_input("设置版本不受支持"));
    }
    let app_state = context.app_state().clone();
    let paused = settings.pause_automatic_backups;
    let repository_changed = app_state
        .repository_path()
        .map_err(CommandError::from)?
        .map(|path| path.to_string_lossy().into_owned())
        != Some(settings.repository_path.trim().to_owned());

    let next = context.run_repository_switch(repository_changed, || {
        let current_repository = app_state.repository_path().map_err(CommandError::from)?;
        let prepared_repository = prepare_repository(
            current_repository.as_deref(),
            settings.repository_path.trim(),
        )
        .map_err(CommandError::from)?;
        write_json_atomic(app_state.settings_path(), &settings).map_err(CommandError::from)?;
        app_state
            .replace_settings(settings)
            .map_err(CommandError::from)?;
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
/// 不阻断设置读取。
pub(crate) fn snapshot_for_command(
    context: &CommandContext,
) -> Result<SettingsSnapshot, CommandError> {
    snapshot(context.app_state()).map_err(CommandError::from)
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
            let result =
                switching.execute(OperationDomain::Settings, |context| save(context, next));
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
