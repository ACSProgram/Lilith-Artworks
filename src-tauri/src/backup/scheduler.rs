use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::{
    app::AppState,
    history::{self, ScheduledBranch},
    storage,
};

use super::{runtime::ExclusiveRunError, worker, BackupCommitResult, BackupState};

const ERROR_RETRY: Duration = Duration::from_secs(60);
const IDLE_RECHECK: Duration = Duration::from_secs(5 * 60);

pub(crate) fn run(state: BackupState, app: AppHandle) {
    loop {
        if !state.wait_scheduler(Duration::ZERO) {
            break;
        }
        let app_state = app.state::<AppState>().inner().clone();
        if app.state::<AppState>().automatic_backups_paused() {
            state.set_automatic_scheduling(false);
            if !state.wait_scheduler(IDLE_RECHECK) {
                break;
            }
            continue;
        }
        state.set_automatic_scheduling(true);
        let quick_default = app_state.automatic_backup_quick_default();
        let branches = match app_state.with_ready_repository(history::list_scheduled) {
            Ok(branches) => branches,
            Err(_) => {
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        let now = match storage::now_ms() {
            Ok(now) => now,
            Err(_) => {
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        let mut due = None;
        let mut next_due = None;
        for branch in branches {
            // 手动提交优先：有待处理手动请求的分支本轮不再自动备份，
            // 由手动提交完成后的调度唤醒接管。
            if state.manual_pending(&branch.id) {
                continue;
            }
            let due_at = due_at_ms(&branch, now);
            if due_at <= now {
                due = Some(branch.id);
                break;
            }
            next_due = Some(next_due.map_or(due_at, |current: i64| current.min(due_at)));
        }
        if let Some(branch_id) = due {
            state.set_active_automatic(true);
            let result = state.run_exclusive_typed(Some(&branch_id), || {
                app_state
                    .with_ready_repository(|root| {
                        Ok(run_scheduled_backup(
                            root,
                            &state,
                            &branch_id,
                            quick_default,
                        ))
                    })
                    .map_err(AutomaticBackupError::Infrastructure)?
            });
            state.set_active_automatic(false);
            match result {
                Ok(_)
                | Err(ExclusiveRunError::Operation(
                    AutomaticBackupError::NotScheduled | AutomaticBackupError::Deferred,
                )) => {}
                Err(ExclusiveRunError::Operation(AutomaticBackupError::Cancelled)) => {
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                }
                Err(ExclusiveRunError::Operation(AutomaticBackupError::Failed {
                    error,
                    disabled,
                })) => {
                    if disabled {
                        log::error!("automatic backup disabled after repeated failures for branch {branch_id}: {error}");
                    } else {
                        log::error!("automatic backup failed for branch {branch_id}: {error}");
                    }
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                }
                Err(ExclusiveRunError::Operation(AutomaticBackupError::Infrastructure(error)))
                | Err(ExclusiveRunError::State(error)) => {
                    log::error!(
                        "automatic backup scheduler failed for branch {branch_id}: {error}"
                    );
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                }
                Err(ExclusiveRunError::ShuttingDown) => break,
            }
            continue;
        }
        let timeout = next_due
            .map(|due_at| Duration::from_millis(due_at.saturating_sub(now) as u64))
            .unwrap_or(IDLE_RECHECK)
            .min(IDLE_RECHECK);
        if !state.wait_scheduler(timeout) {
            break;
        }
    }
}

#[derive(Debug)]
enum AutomaticBackupError {
    NotScheduled,
    /// 分支有待处理的手动提交，本轮自动备份主动让位，不计入失败。
    Deferred,
    Cancelled,
    Failed {
        error: String,
        disabled: bool,
    },
    Infrastructure(String),
}

impl std::fmt::Display for AutomaticBackupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotScheduled => formatter.write_str("分支不再满足自动备份条件"),
            Self::Deferred => formatter.write_str("分支有待处理的手动提交，自动备份已延后"),
            Self::Cancelled => formatter.write_str("自动备份已取消"),
            Self::Failed { error, .. } | Self::Infrastructure(error) => error.fmt(formatter),
        }
    }
}

fn run_scheduled_backup(
    root: &std::path::Path,
    state: &BackupState,
    branch_id: &str,
    quick_default: bool,
) -> Result<BackupCommitResult, AutomaticBackupError> {
    let now = storage::now_ms().map_err(AutomaticBackupError::Infrastructure)?;
    let branch = history::load_scheduled(root, branch_id)
        .map_err(AutomaticBackupError::Infrastructure)?
        .ok_or(AutomaticBackupError::NotScheduled)?;
    if due_at_ms(&branch, now) > now {
        return Err(AutomaticBackupError::NotScheduled);
    }
    // 取得运行锁后再次确认没有待处理的手动提交，防止手动请求在
    // 候选选择与加锁之间到达时仍被自动任务抢先。
    if state.manual_pending(branch_id) {
        return Err(AutomaticBackupError::Deferred);
    }
    // 快速检查：分支单独开启或全局默认为快速时，先只比较大小与修改时间；
    // 完全一致视为内容未变化，任何不一致都退回下方全量检查和备份流程。
    if branch.quick_enabled || quick_default {
        let quick_unchanged = worker::quick_check_unchanged(root, branch_id).map_err(|error| {
            AutomaticBackupError::Failed {
                error,
                disabled: false,
            }
        })?;
        if quick_unchanged {
            let checked_ms = storage::now_ms().map_err(AutomaticBackupError::Infrastructure)?;
            history::mark_unchanged(root, branch_id, checked_ms)
                .map_err(AutomaticBackupError::Infrastructure)?;
            log::info!("backup quick-unchanged: branch_id={branch_id}");
            return Ok(BackupCommitResult {
                created: false,
                unchanged: true,
                history_id: None,
            });
        }
    }
    match worker::run_backup(root, branch_id, "", "automatic", || state.cancelled()) {
        Ok(result) => Ok(result),
        Err(worker::BackupRunError::Cancelled) => Err(AutomaticBackupError::Cancelled),
        Err(worker::BackupRunError::Failed(error)) => {
            let failed_ms = storage::now_ms().map_err(AutomaticBackupError::Infrastructure)?;
            let disabled = history::mark_automatic_backup_error(root, branch_id, &error, failed_ms)
                .map_err(AutomaticBackupError::Infrastructure)?;
            Err(AutomaticBackupError::Failed { error, disabled })
        }
    }
}

fn due_at_ms(branch: &ScheduledBranch, now: i64) -> i64 {
    branch.retry_at_ms.unwrap_or_else(|| {
        branch.last_check_ms.map_or(now, |checked| {
            checked
                .saturating_add(i64::from(branch.interval_minutes) * 60_000)
                .saturating_add(jitter_ms(&branch.id))
        })
    })
}

fn jitter_ms(branch_id: &str) -> i64 {
    let hash = branch_id.bytes().fold(0_u64, |value, byte| {
        value.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    ((hash % 21) as i64 - 10) * 1_000
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, path::Path};

    use super::*;

    fn make_due(root: &Path, branch_id: &str) {
        crate::storage::open(root)
            .unwrap()
            .execute(
                "UPDATE branches SET last_check_ms = 1, backup_retry_at_ms = NULL WHERE id = ?1",
                [branch_id],
            )
            .unwrap();
    }

    fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
        String,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.bin");
        fs::File::create(&source)
            .unwrap()
            .write_all(b"scheduled content")
            .unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork =
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source).unwrap();
        (
            directory,
            root,
            source,
            artwork.artwork_id,
            artwork.branch_id,
        )
    }

    #[test]
    fn quick_check_marks_unchanged_without_creating_a_node() {
        let (_directory, root, _source, artwork_id, branch_id) = fixture();
        worker::run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        make_due(&root, &branch_id);

        let result =
            run_scheduled_backup(&root, &BackupState::default(), &branch_id, true).unwrap();

        assert!(!result.created);
        assert!(result.unchanged);
        assert_eq!(history::list(&root, &artwork_id).unwrap().nodes.len(), 1);
        let last_check: i64 = crate::storage::open(&root)
            .unwrap()
            .query_row(
                "SELECT last_check_ms FROM branches WHERE id = ?1",
                [&branch_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_check > 1);
    }

    #[test]
    fn quick_check_falls_back_to_full_backup_when_metadata_changes() {
        let (_directory, root, source, artwork_id, branch_id) = fixture();
        worker::run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        make_due(&root, &branch_id);
        fs::write(&source, b"changed content").unwrap();

        let result =
            run_scheduled_backup(&root, &BackupState::default(), &branch_id, true).unwrap();

        assert!(result.created);
        assert_eq!(history::list(&root, &artwork_id).unwrap().nodes.len(), 2);
    }

    #[test]
    fn branch_quick_flag_uses_quick_path_even_with_full_default() {
        let (_directory, root, _source, _artwork_id, branch_id) = fixture();
        let first_id = worker::run_backup(&root, &branch_id, "First", "manual", || false)
            .unwrap()
            .history_id
            .unwrap();
        history::update_branch(&root, &branch_id, "Main", true, true, 10, true, None).unwrap();
        // 破坏 head snapshot：全量路径会核对并修复，快速路径应完全跳过校验。
        let original_relative = history::load_node(&root, &first_id)
            .unwrap()
            .snapshot_path
            .unwrap();
        let original_path = crate::storage::resolve_path(&root, &original_relative).unwrap();
        let mut damaged = fs::read(&original_path).unwrap();
        *damaged.last_mut().unwrap() ^= 0xff;
        fs::write(&original_path, damaged).unwrap();
        make_due(&root, &branch_id);

        let result =
            run_scheduled_backup(&root, &BackupState::default(), &branch_id, false).unwrap();

        assert!(!result.created);
        assert!(result.unchanged);
        assert_eq!(
            history::load_node(&root, &first_id).unwrap().snapshot_path,
            Some(original_relative)
        );
    }

    #[test]
    fn scheduled_backup_defers_to_a_pending_manual_request() {
        let (_directory, root, _source, _artwork_id, branch_id) = fixture();
        worker::run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        make_due(&root, &branch_id);
        let state = BackupState::default();
        state.begin_manual_request(&branch_id);

        let deferred = run_scheduled_backup(&root, &state, &branch_id, true);

        assert!(matches!(deferred, Err(AutomaticBackupError::Deferred)));
        state.end_manual_request(&branch_id);
        let result = run_scheduled_backup(&root, &state, &branch_id, true).unwrap();
        assert!(result.unchanged);
    }

    #[test]
    fn execution_recheck_skips_a_branch_disabled_after_selection() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.bin");
        fs::File::create(&source)
            .unwrap()
            .write_all(b"scheduled content")
            .unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork =
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source).unwrap();
        assert!(history::load_scheduled(&root, &artwork.branch_id)
            .unwrap()
            .is_some());

        history::update_branch(
            &root,
            &artwork.branch_id,
            "Main",
            true,
            false,
            10,
            false,
            None,
        )
        .unwrap();
        let result =
            run_scheduled_backup(&root, &BackupState::default(), &artwork.branch_id, false);

        assert!(matches!(result, Err(AutomaticBackupError::NotScheduled)));
        let branch = history::list(&root, &artwork.artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.consecutive_backup_failures, 0);
        assert!(branch.last_error.is_none());
    }

    #[test]
    fn due_time_uses_retry_deadline_without_interval_jitter() {
        let branch = ScheduledBranch {
            id: "branch".into(),
            last_check_ms: Some(10),
            interval_minutes: 120,
            quick_enabled: false,
            retry_at_ms: Some(42),
        };
        assert_eq!(due_at_ms(&branch, 1_000), 42);
    }

    #[test]
    fn execution_recheck_skips_a_branch_that_entered_publication() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.bin");
        let artifact = directory.path().join("final.png");
        fs::write(&source, b"scheduled content").unwrap();
        fs::write(&artifact, b"final content").unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork =
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source).unwrap();
        let history_id =
            worker::run_backup(&root, &artwork.branch_id, "Initial", "manual", || false)
                .unwrap()
                .history_id
                .unwrap();
        crate::authenticity::store_final_artifact(
            &root,
            &artwork.branch_id,
            &history_id,
            artifact.to_str().unwrap(),
        )
        .unwrap();

        let result =
            run_scheduled_backup(&root, &BackupState::default(), &artwork.branch_id, false);

        assert!(matches!(result, Err(AutomaticBackupError::NotScheduled)));
        let branch = history::list(&root, &artwork.artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.consecutive_backup_failures, 0);
        assert!(branch.last_error.is_none());
    }

    #[test]
    fn cancelled_scheduled_backup_does_not_count_as_a_failure() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let source = directory.path().join("artwork.bin");
        fs::write(&source, b"scheduled content").unwrap();
        crate::library::initialize(&root).unwrap();
        let artwork =
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source).unwrap();
        let state = BackupState::default();
        let operation_state = state.clone();

        let result = state.run_exclusive_typed(Some(&artwork.branch_id), || {
            assert!(operation_state.request_cancel().unwrap());
            run_scheduled_backup(&root, &operation_state, &artwork.branch_id, false)
        });

        assert!(matches!(
            result,
            Err(ExclusiveRunError::Operation(
                AutomaticBackupError::Cancelled
            ))
        ));
        let branch = history::list(&root, &artwork.artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.consecutive_backup_failures, 0);
        assert!(branch.last_error.is_none());
        assert!(branch.backup_enabled);
    }
}
