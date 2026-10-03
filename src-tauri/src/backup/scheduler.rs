use std::{path::Path, time::Duration};

use tauri::{AppHandle, Manager};

use crate::{
    app::AppState,
    history::{self, IdleVerifyTarget, ScheduledBranch},
    storage,
};

use super::{
    runtime::ExclusiveRunError, validate_snapshot, worker, BackupCommitResult, BackupState,
    BackupTaskKind,
};

const ERROR_RETRY: Duration = Duration::from_secs(60);
const IDLE_RECHECK: Duration = Duration::from_secs(5 * 60);
/// 有前台命令在等待共享运行锁时的让位退避。它足够短以保证前台结束后的响应性，
/// 又足以避免调度器空转抢锁。
const FOREGROUND_YIELD_BACKOFF: Duration = Duration::from_millis(200);
/// 空闲链路校验前，分支 head 至少需要静默的时长。避免刚提交就整链重读；期间又提交
/// 会改变 head，本轮随之让位给下一次。取值 10 分钟（见规划 4.3，可由维护者调整）。
const IDLE_VERIFY_DELAY_MS: i64 = 10 * 60 * 1000;

pub(crate) fn run(state: BackupState, app: AppHandle) {
    loop {
        if !state.wait_scheduler(Duration::ZERO) {
            break;
        }
        let app_state = app.state::<AppState>().inner().clone();
        // 托盘"暂停所有自动备份"同时暂停空闲校验：用户意图是"别动仓库"。
        let paused = app.state::<AppState>().automatic_backups_paused();
        if paused {
            state.set_automatic_scheduling(false);
            if !state.wait_scheduler(IDLE_RECHECK) {
                break;
            }
            continue;
        }
        state.set_automatic_scheduling(true);
        // 有前台命令正在等待共享运行锁时主动让位：不选新任务，也不与前台争抢
        // 运行锁。
        if state.foreground_waiting() > 0 {
            if !state.wait_scheduler(FOREGROUND_YIELD_BACKOFF) {
                break;
            }
            continue;
        }
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
            let result = state.run_exclusive_typed(
                Some(&branch_id),
                BackupTaskKind::AutomaticBackup,
                || {
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
                },
            );
            match result {
                Ok(_)
                | Err(ExclusiveRunError::Operation(
                    AutomaticBackupError::NotScheduled
                    | AutomaticBackupError::Deferred
                    | AutomaticBackupError::ForegroundWaiting,
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
        // 第二优先级：空闲链路校验。第一优先级没有到期备份时才考虑，且整体让位于
        // 暂停、前台等待与待处理的手动提交。每轮只处理一个分支，处理完回到循环顶部
        // 重新评估优先级，保证到期备份与前台操作的响应性。
        let verify_candidates =
            match app_state.with_ready_repository(history::list_idle_verify_targets) {
                Ok(targets) => targets,
                Err(_) => {
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                    continue;
                }
            };
        let selection = select_idle_verify(
            &verify_candidates,
            now,
            paused,
            state.foreground_waiting(),
            state.any_manual_pending(),
        );
        if let Some(target) = selection.ready {
            let branch_id = target.branch_id.clone();
            let expected_head = target.head_history_id.clone();
            let result =
                state.run_exclusive_typed(Some(&branch_id), BackupTaskKind::IdleVerify, || {
                    app_state
                        .with_ready_repository(|root| {
                            Ok(run_idle_verify(root, &state, &branch_id, &expected_head))
                        })
                        .map_err(IdleVerifyError::Infrastructure)?
                });
            match result {
                Ok(_)
                | Err(ExclusiveRunError::Operation(
                    IdleVerifyError::NotPending
                    | IdleVerifyError::Deferred
                    | IdleVerifyError::ForegroundWaiting,
                )) => {}
                Err(ExclusiveRunError::Operation(IdleVerifyError::Cancelled)) => {
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                }
                // 校验失败已经写入 verify_error 使分支退出队列，只需记录警告；
                // 不进入自动备份的失败退避，也不阻断后续分支。
                Err(ExclusiveRunError::Operation(IdleVerifyError::Failed(error))) => {
                    log::warn!("idle verify failed for branch {branch_id}: {error}");
                }
                Err(ExclusiveRunError::Operation(IdleVerifyError::Infrastructure(error)))
                | Err(ExclusiveRunError::State(error)) => {
                    log::error!("idle verify scheduler failed for branch {branch_id}: {error}");
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                }
                Err(ExclusiveRunError::ShuttingDown) => break,
            }
            continue;
        }
        let timeout = if selection.blocked {
            // 让位期间由唤醒信号驱动恢复（手动提交结束会唤醒），不必空转抢锁。
            IDLE_RECHECK
        } else {
            next_due
                .into_iter()
                .chain(selection.next_ready_at)
                .min()
                .map(|at| Duration::from_millis(at.saturating_sub(now) as u64))
                .unwrap_or(IDLE_RECHECK)
                .min(IDLE_RECHECK)
        };
        if !state.wait_scheduler(timeout) {
            break;
        }
    }
}

/// 空闲校验的第二优先级选择结果。
#[derive(Default)]
struct IdleVerifySelection {
    /// 选取到的候选：head 已静默满 `IDLE_VERIFY_DELAY_MS`，可以立即校验。
    ready: Option<IdleVerifyTarget>,
    /// 候选中最接近可校验的时刻；`blocked` 或无候选时为 `None`。
    next_ready_at: Option<i64>,
    /// 第二优先级整体让位（暂停、有前台命令等待，或存在待处理手动提交）。
    blocked: bool,
}

/// 第二优先级选择：从派生队列里挑一条 head 已静默至少 `IDLE_VERIFY_DELAY_MS`
/// 的分支。暂停、有前台命令等待、或存在待处理手动提交时整体让位，返回 `blocked`。
fn select_idle_verify(
    candidates: &[IdleVerifyTarget],
    now: i64,
    paused: bool,
    foreground_waiting: usize,
    manual_pending: bool,
) -> IdleVerifySelection {
    if candidates.is_empty() {
        return IdleVerifySelection::default();
    }
    if paused || foreground_waiting > 0 || manual_pending {
        return IdleVerifySelection {
            ready: None,
            next_ready_at: None,
            blocked: true,
        };
    }
    let mut next_ready_at = None;
    for candidate in candidates {
        let ready_at = candidate
            .head_created_ms
            .saturating_add(IDLE_VERIFY_DELAY_MS);
        if ready_at <= now {
            return IdleVerifySelection {
                ready: Some(candidate.clone()),
                next_ready_at: None,
                blocked: false,
            };
        }
        next_ready_at = Some(next_ready_at.map_or(ready_at, |current: i64| current.min(ready_at)));
    }
    IdleVerifySelection {
        ready: None,
        next_ready_at,
        blocked: false,
    }
}

/// 校验分支 head 的单个 snapshot。
///
/// `materialization_chain` 在目标节点持有 snapshot 时只返回该节点，因此校验 head
/// 等价于校验它的一个 snapshot 文件；这里不分块回溯整条链，也不新增依赖。
/// 取得运行锁后复查 head 仍与候选快照一致，并用运行状态暴露进度与取消。
fn run_idle_verify(
    root: &Path,
    state: &BackupState,
    branch_id: &str,
    expected_head: &str,
) -> Result<(), IdleVerifyError> {
    // 与到期自动备份相同的锁内让位复查：手动提交优先、前台命令优先。
    if state.manual_pending(branch_id) {
        return Err(IdleVerifyError::Deferred);
    }
    if state.foreground_waiting() > 0 {
        return Err(IdleVerifyError::ForegroundWaiting);
    }
    let target = history::load_idle_verify_target(root, branch_id)
        .map_err(IdleVerifyError::Infrastructure)?
        .ok_or(IdleVerifyError::NotPending)?;
    if target.head_history_id != expected_head {
        // 候选选择与取得运行锁之间 head 已经变化：本轮不写结果，留给下一次。
        return Err(IdleVerifyError::NotPending);
    }
    if state.cancelled() {
        return Err(IdleVerifyError::Cancelled);
    }
    let record = history::load_node(root, &target.head_history_id)
        .map_err(IdleVerifyError::Infrastructure)?;
    let relative = record
        .snapshot_path
        .as_deref()
        .ok_or_else(|| IdleVerifyError::Failed("分支 head 缺少 snapshot，无法校验".into()))?;
    let path = storage::resolve_path(root, relative).map_err(IdleVerifyError::Infrastructure)?;
    state.report_progress("verify", "正在校验分支链路", 0, 1);
    match validate_snapshot(&path, &record, "链路校验") {
        Ok(()) => {
            let verified_ms = storage::now_ms().map_err(IdleVerifyError::Infrastructure)?;
            history::mark_verified(root, branch_id, &target.head_history_id, verified_ms)
                .map_err(IdleVerifyError::Infrastructure)?;
            log::info!(
                "idle verify passed: branch_id={branch_id}, head={}",
                target.head_history_id
            );
            Ok(())
        }
        Err(error) => {
            history::mark_verify_error(root, branch_id, &target.head_history_id, &error)
                .map_err(IdleVerifyError::Infrastructure)?;
            Err(IdleVerifyError::Failed(error))
        }
    }
}

#[derive(Debug)]
enum AutomaticBackupError {
    NotScheduled,
    /// 分支有待处理的手动提交，本轮自动备份主动让位，不计入失败。
    Deferred,
    /// 有前台命令在等待共享运行锁，本轮自动备份主动让位，不计入失败。
    ForegroundWaiting,
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
            Self::ForegroundWaiting => formatter.write_str("有前台命令在等待，自动备份已让位"),
            Self::Cancelled => formatter.write_str("自动备份已取消"),
            Self::Failed { error, .. } | Self::Infrastructure(error) => error.fmt(formatter),
        }
    }
}

#[derive(Debug)]
enum IdleVerifyError {
    /// 分支已不再待校验（已校验、已进回收站，或 head 在候选选择后变化）。
    NotPending,
    /// 分支有待处理的手动提交，本轮让位，不计入失败。
    Deferred,
    /// 有前台命令在等待共享运行锁，本轮让位，不计入失败。
    ForegroundWaiting,
    Cancelled,
    /// 校验失败（摘要已写入 `verify_error`）；不重试，也不影响备份。
    Failed(String),
    Infrastructure(String),
}

impl std::fmt::Display for IdleVerifyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPending => formatter.write_str("分支已不再待校验"),
            Self::Deferred => formatter.write_str("分支有待处理的手动提交，空闲校验已让位"),
            Self::ForegroundWaiting => formatter.write_str("有前台命令在等待，空闲校验已让位"),
            Self::Cancelled => formatter.write_str("空闲校验已取消"),
            Self::Failed(error) | Self::Infrastructure(error) => error.fmt(formatter),
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
    // 取得运行锁后复查前台等待登记：有前台命令在等锁时立即让位，既不复核
    // 资格也不占用仓库锁，保证前台命令尽快拿到运行锁。
    if state.foreground_waiting() > 0 {
        return Err(AutomaticBackupError::ForegroundWaiting);
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
    fn scheduled_backup_yields_to_a_waiting_foreground_command() {
        let (_directory, root, _source, artwork_id, branch_id) = fixture();
        worker::run_backup(&root, &branch_id, "First", "manual", || false).unwrap();
        make_due(&root, &branch_id);
        let state = BackupState::default();
        // 模拟一个已登记、仍在等待运行锁的前台命令：后台任务取得运行锁后必须让位，
        // 且不计入失败、不推进检查时间。
        let waiting = state.begin_foreground_wait();

        let yielded = run_scheduled_backup(&root, &state, &branch_id, true);

        assert!(
            matches!(yielded, Err(AutomaticBackupError::ForegroundWaiting)),
            "{yielded:?}"
        );
        drop(waiting);
        assert_eq!(state.foreground_waiting(), 0);
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.consecutive_backup_failures, 0);
        assert!(branch.last_error.is_none());

        let result = run_scheduled_backup(&root, &state, &branch_id, true).unwrap();
        assert!(result.unchanged);
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

        let result = state.run_exclusive_typed(
            Some(&artwork.branch_id),
            BackupTaskKind::AutomaticBackup,
            || {
                assert!(operation_state.request_cancel().unwrap());
                run_scheduled_backup(&root, &operation_state, &artwork.branch_id, false)
            },
        );

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

    fn verify_target(branch_id: &str, head_created_ms: i64) -> IdleVerifyTarget {
        IdleVerifyTarget {
            branch_id: branch_id.into(),
            head_history_id: "head".into(),
            head_created_ms,
        }
    }

    #[test]
    fn idle_verify_selection_waits_for_the_head_to_settle() {
        let now = 1_000_000;
        let fresh = verify_target("fresh", now - 1_000);
        let selection = select_idle_verify(&[fresh], now, false, 0, false);
        assert!(selection.ready.is_none());
        assert_eq!(
            selection.next_ready_at,
            Some(now - 1_000 + IDLE_VERIFY_DELAY_MS)
        );

        let settled = verify_target("settled", now - IDLE_VERIFY_DELAY_MS - 1);
        let selection = select_idle_verify(&[settled], now, false, 0, false);
        assert_eq!(
            selection.ready.map(|target| target.branch_id),
            Some("settled".into())
        );
        assert!(selection.next_ready_at.is_none());
    }

    #[test]
    fn idle_verify_selection_yields_while_busy_or_paused() {
        let now = 1_000_000;
        let settled = verify_target("settled", now - IDLE_VERIFY_DELAY_MS - 1);
        // 暂停所有自动备份时空闲校验一并暂停。
        assert!(select_idle_verify(&[settled.clone()], now, true, 0, false).blocked);
        // 有前台命令在等待。
        assert!(select_idle_verify(&[settled.clone()], now, false, 1, false).blocked);
        // 存在待处理的手动提交。
        assert!(select_idle_verify(&[settled], now, false, 0, true).blocked);
    }

    #[test]
    fn idle_verify_selection_ignores_an_empty_queue() {
        let selection = select_idle_verify(&[], 1_000, false, 0, false);
        assert!(selection.ready.is_none());
        assert!(selection.next_ready_at.is_none());
        assert!(!selection.blocked);
    }

    #[test]
    fn a_branch_without_head_is_not_queued_for_verification() {
        let (_directory, root, _source, _artwork_id, _branch_id) = fixture();
        // 已建分支但尚未提交：head 为空，派生队列必须为空，避免每轮空跑。
        assert!(history::list_idle_verify_targets(&root).unwrap().is_empty());
    }

    #[test]
    fn idle_verify_marks_a_healthy_head_verified_and_leaves_the_queue() {
        let (_directory, root, _source, artwork_id, branch_id) = fixture();
        let history_id = worker::run_backup(&root, &branch_id, "First", "manual", || false)
            .unwrap()
            .history_id
            .unwrap();
        let targets = history::list_idle_verify_targets(&root).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].head_history_id, history_id);

        run_idle_verify(&root, &BackupState::default(), &branch_id, &history_id).unwrap();

        assert!(history::list_idle_verify_targets(&root).unwrap().is_empty());
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch.verify_error.is_none());
        assert!(branch.verified_ms.is_some());
        assert_eq!(branch.consecutive_backup_failures, 0);
        assert!(branch.last_error.is_none());
    }

    #[test]
    fn idle_verify_records_a_failure_and_retries_only_after_requeue() {
        let (_directory, root, _source, artwork_id, branch_id) = fixture();
        let history_id = worker::run_backup(&root, &branch_id, "First", "manual", || false)
            .unwrap()
            .history_id
            .unwrap();
        // 破坏 head snapshot 内容，摘要与数据库记录不再匹配。
        let record = history::load_node(&root, &history_id).unwrap();
        let path = storage::resolve_path(&root, record.snapshot_path.as_deref().unwrap()).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&path, bytes).unwrap();

        let failed = run_idle_verify(&root, &BackupState::default(), &branch_id, &history_id);
        assert!(
            matches!(failed, Err(IdleVerifyError::Failed(_))),
            "{failed:?}"
        );

        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch.verify_error.is_some());
        assert!(branch.verified_ms.is_none());
        // 失败不自动重试：分支退出派生队列。
        assert!(history::list_idle_verify_targets(&root).unwrap().is_empty());

        // 用户手动重查：清空失败摘要后重新入队。
        history::clear_verify_error(&root, &branch_id).unwrap();
        assert_eq!(history::list_idle_verify_targets(&root).unwrap().len(), 1);
    }

    #[test]
    fn a_new_head_requeues_verification_and_clears_the_previous_failure() {
        let (_directory, root, source, artwork_id, branch_id) = fixture();
        let first_id = worker::run_backup(&root, &branch_id, "First", "manual", || false)
            .unwrap()
            .history_id
            .unwrap();
        history::mark_verify_error(&root, &branch_id, &first_id, "链路校验失败").unwrap();
        assert!(history::list_idle_verify_targets(&root).unwrap().is_empty());

        // 新提交产生新 head：分支重新入队，且旧失败摘要被清空。
        fs::write(&source, b"changed content").unwrap();
        let second_id = worker::run_backup(&root, &branch_id, "Second", "manual", || false)
            .unwrap()
            .history_id
            .unwrap();
        assert_ne!(first_id, second_id);

        let targets = history::list_idle_verify_targets(&root).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].head_history_id, second_id);
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch.verify_error.is_none());
    }

    #[test]
    fn idle_verify_skips_a_head_that_changed_after_selection() {
        let (_directory, root, _source, artwork_id, branch_id) = fixture();
        worker::run_backup(&root, &branch_id, "First", "manual", || false).unwrap();

        let result = run_idle_verify(
            &root,
            &BackupState::default(),
            &branch_id,
            "head-from-a-stale-snapshot",
        );
        assert!(
            matches!(result, Err(IdleVerifyError::NotPending)),
            "{result:?}"
        );
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch.verify_error.is_none());
        assert!(branch.verified_ms.is_none());
    }

    #[test]
    fn idle_verify_yields_to_foreground_and_manual_work() {
        let (_directory, root, _source, _artwork_id, branch_id) = fixture();
        let history_id = worker::run_backup(&root, &branch_id, "First", "manual", || false)
            .unwrap()
            .history_id
            .unwrap();
        let state = BackupState::default();

        let waiting = state.begin_foreground_wait();
        let yielded = run_idle_verify(&root, &state, &branch_id, &history_id);
        assert!(
            matches!(yielded, Err(IdleVerifyError::ForegroundWaiting)),
            "{yielded:?}"
        );
        drop(waiting);

        state.begin_manual_request(&branch_id);
        let deferred = run_idle_verify(&root, &state, &branch_id, &history_id);
        assert!(matches!(deferred, Err(IdleVerifyError::Deferred)));
        state.end_manual_request(&branch_id);

        assert!(run_idle_verify(&root, &state, &branch_id, &history_id).is_ok());
    }
}
