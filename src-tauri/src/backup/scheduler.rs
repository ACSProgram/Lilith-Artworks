use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
        Arc,
    },
    time::Duration,
};

use tauri::{AppHandle, Manager};

use crate::{
    app::AppState,
    cancel::CancelToken,
    history::{self, IdleVerifyTarget, ScheduledBranch},
    repository_lock::{self, OwnerLockError, RepositoryOwnerLock},
    storage,
};

use super::chunk_file::ChunkWriteOptions;

use super::{
    runtime::ExclusiveRunError, validate_snapshot, worker, BackupCommitResult, BackupState,
    BackupTaskKind,
};

const ERROR_RETRY: Duration = Duration::from_secs(60);
const IDLE_RECHECK: Duration = Duration::from_secs(5 * 60);
/// 有前台命令在等待共享运行锁时的让位退避。它足够短以保证前台结束后的响应性，
/// 又足以避免调度器空转抢锁。
const FOREGROUND_YIELD_BACKOFF: Duration = Duration::from_millis(200);
/// 用户取消后台任务后的短暂退避：不把整支队列一起停 60 秒，也避免立刻重跑。
const CANCEL_RETRY: Duration = Duration::from_secs(5);
/// 空闲链路校验前，分支 head 至少需要静默的时长。避免刚提交就整链重读；期间又提交
/// 会改变 head，本轮随之让位给下一次。取值 10 分钟（见规划 4.3，可由维护者调整）。
const IDLE_VERIFY_DELAY_MS: i64 = 10 * 60 * 1000;
/// aging（防饥饿）：每连续完成 N 次自动备份后，若已有就绪的链路校验，先让校验
/// 跑一轮。维护者 2026-10-18 定稿为按次数、N = 4，不引入时钟规则。
const IDLE_VERIFY_AGING_RUNS: u32 = 4;
/// 并行只读校验上限。按 Artwork 分片，同一作品同时最多一个校验任务；写回结果
/// 仍由共享运行锁串行化。
const IDLE_VERIFY_WORKERS: usize = 4;

pub(crate) fn run(state: BackupState, app: AppHandle) {
    let mut automatic_since_verify = 0_u32;
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
        // 前台等待与手动提交都高于所有后台任务：先让出，等前台结束后的唤醒信号。
        if state.foreground_waiting() > 0 || state.any_manual_pending() {
            if !state.wait_scheduler(FOREGROUND_YIELD_BACKOFF) {
                break;
            }
            continue;
        }
        // 候选筛选会顺带写「自动备份被跳过」的原因，因此同样按一次写操作取得
        // 跨进程 owner lock。
        let candidates_owner = match acquire_owner(&app_state, "backup:candidates") {
            Ok(owner) => owner,
            Err(error) => {
                log::warn!(
                    "automatic backup scheduler could not take the repository owner lock: {error}"
                );
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        let branches = match app_state.with_ready_repository(|root| {
            let branches = history::list_scheduled(root)?;
            // 网络位置与设备命名空间的工作文件不进入无人值守流程：快速检查会直接
            // 对这条路径取元数据，全量备份还会打开并整份复制它。这里把原因写进
            // 分支记录，与「未选择工作文件的分支不参与自动备份」同一口径；
            // `mark_automatic_backup_skipped` 在原因未变时不写库，因此每轮调用
            // 不会产生写入。
            for branch in &branches {
                let Some(reason) = storage::unattended_source_path_rejection(&branch.source_path)
                else {
                    continue;
                };
                if let Err(error) =
                    history::mark_automatic_backup_skipped(root, &branch.id, &reason)
                {
                    log::warn!(
                        "failed to record skipped automatic backup for branch {}: {error}",
                        branch.id
                    );
                }
            }
            Ok(branches)
        }) {
            Ok(branches) => branches,
            Err(_) => {
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        drop(candidates_owner);
        let now = match storage::now_ms() {
            Ok(now) => now,
            Err(_) => {
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        // deadline 优先：候选按 due_at、上次检查时间与分支 ID 排序；失败退避已
        // 写在分支自己的 retry_at 里，因此一个分支失败不会阻塞其它到期分支。
        let (due, next_due) = select_due_branch(branches, now, &state);
        let verify_candidates =
            match app_state.with_repository_read(|root| history::list_idle_verify_targets(root)) {
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
            IDLE_VERIFY_WORKERS,
        );
        let verify_ready = !selection.ready.is_empty();
        if verify_precedes_backup(due.is_some(), verify_ready, automatic_since_verify) {
            match run_verify_batch(&app_state, &state, &selection.ready) {
                Ok(()) => automatic_since_verify = 0,
                Err(VerifyBatchError::ShuttingDown) => break,
                Err(VerifyBatchError::Busy(error)) => {
                    log::debug!("idle verify yielded to a repository writer: {error}");
                    if !state.wait_scheduler(FOREGROUND_YIELD_BACKOFF) {
                        break;
                    }
                }
                Err(VerifyBatchError::Infrastructure(error)) => {
                    log::error!("idle verify batch failed: {error}");
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                }
            }
            continue;
        }

        if let Some(branch_id) = due {
            let backup_owner = match acquire_owner(&app_state, "backup:automatic") {
                Ok(owner) => owner,
                Err(error) => {
                    log::warn!(
                        "automatic backup could not take the repository owner lock for branch {branch_id}: {error}"
                    );
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                    continue;
                }
            };
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
                                app_state.chunk_write_options(),
                            ))
                        })
                        .map_err(AutomaticBackupError::Infrastructure)?
                },
            );
            drop(backup_owner);
            match result {
                // aging 只统计真正完成的自动备份；让位、跳过与失败都不推进计数。
                Ok(_) => {
                    automatic_since_verify = automatic_since_verify.saturating_add(1);
                }
                Err(ExclusiveRunError::Operation(
                    AutomaticBackupError::NotScheduled
                    | AutomaticBackupError::Skipped
                    | AutomaticBackupError::Deferred
                    | AutomaticBackupError::ForegroundWaiting,
                )) => {}
                Err(ExclusiveRunError::Operation(AutomaticBackupError::Cancelled)) => {
                    if !state.wait_scheduler(CANCEL_RETRY) {
                        break;
                    }
                }
                // 单分支失败已经按该分支写入 retry_at；立即回到循环顶部处理其它
                // 到期分支，不把整支队列固定停 60 秒。
                Err(ExclusiveRunError::Operation(AutomaticBackupError::Failed {
                    error,
                    disabled,
                })) => {
                    if disabled {
                        log::error!("automatic backup disabled after repeated failures for branch {branch_id}: {error}");
                    } else {
                        log::error!("automatic backup failed for branch {branch_id}: {error}");
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

/// 一条按截止时间排序的自动备份候选。`Ord` 反向实现：`BinaryHeap` 的 `peek`
/// 返回最早到期的分支；同一时刻按上次检查时间、分支 ID 稳定打破平局。
struct DeadlineEntry {
    due_at: i64,
    last_check_ms: i64,
    branch: ScheduledBranch,
}

impl PartialEq for DeadlineEntry {
    fn eq(&self, other: &Self) -> bool {
        self.due_at == other.due_at
            && self.last_check_ms == other.last_check_ms
            && self.branch.id == other.branch.id
    }
}

impl Eq for DeadlineEntry {}

impl Ord for DeadlineEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .due_at
            .cmp(&self.due_at)
            .then_with(|| other.last_check_ms.cmp(&self.last_check_ms))
            .then_with(|| other.branch.id.cmp(&self.branch.id))
    }
}

impl PartialOrd for DeadlineEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 从候选集合中取出最早到期的一条；同时给出下一次到期时刻。
fn select_due_branch(
    branches: Vec<ScheduledBranch>,
    now: i64,
    state: &BackupState,
) -> (Option<String>, Option<i64>) {
    let mut heap = BinaryHeap::new();
    for branch in branches {
        // 手动提交优先：该分支本轮不自动备份，由手动提交完成后的唤醒接管。
        if state.manual_pending(&branch.id) {
            continue;
        }
        // 网络位置与设备命名空间的工作文件不进入无人值守流程（原因已在候选
        // 筛选时写入分支记录），这里只跳过，不参与到期判定。
        if storage::unattended_source_path_rejection(&branch.source_path).is_some() {
            continue;
        }
        let due_at = due_at_ms(&branch, now);
        heap.push(DeadlineEntry {
            due_at,
            last_check_ms: branch.last_check_ms.unwrap_or(0),
            branch,
        });
    }
    let next_due = heap.peek().map(|entry| entry.due_at);
    let due = if heap.peek().is_some_and(|entry| entry.due_at <= now) {
        heap.pop().map(|entry| entry.branch.id)
    } else {
        None
    };
    (due, next_due)
}

/// 第二优先级是否先于本轮自动备份执行：没有到期备份时始终优先；有到期备份时
/// 仅在连续 `IDLE_VERIFY_AGING_RUNS` 次自动备份后插队一次，避免校验被持续
/// 到期的备份饿死。
fn verify_precedes_backup(due: bool, verify_ready: bool, automatic_since_verify: u32) -> bool {
    verify_ready && (!due || automatic_since_verify >= IDLE_VERIFY_AGING_RUNS)
}

/// 空闲校验的第二优先级选择结果。
#[derive(Default)]
struct IdleVerifySelection {
    /// 选取到的候选：head 已静默满 `IDLE_VERIFY_DELAY_MS`，可以立即校验。
    /// 按 Artwork 去重，最多 `limit` 条并行准备。
    ready: Vec<IdleVerifyTarget>,
    /// 候选中最接近可校验的时刻；已有 ready 或 `blocked` 时为 `None`。
    next_ready_at: Option<i64>,
    /// 第二优先级整体让位（暂停、有前台命令等待，或存在待处理手动提交）。
    blocked: bool,
}

/// 第二优先级选择：从派生队列里挑一批 head 已静默至少 `IDLE_VERIFY_DELAY_MS`
/// 的分支，按 Artwork 去重。暂停、有前台命令等待、或存在待处理手动提交时整体
/// 让位，返回 `blocked`。
fn select_idle_verify(
    candidates: &[IdleVerifyTarget],
    now: i64,
    paused: bool,
    foreground_waiting: usize,
    manual_pending: bool,
    limit: usize,
) -> IdleVerifySelection {
    if candidates.is_empty() {
        return IdleVerifySelection::default();
    }
    if paused || foreground_waiting > 0 || manual_pending {
        return IdleVerifySelection {
            ready: Vec::new(),
            next_ready_at: None,
            blocked: true,
        };
    }
    let mut next_ready_at = None;
    let mut ready = Vec::new();
    let mut artworks = HashSet::new();
    for candidate in candidates {
        let ready_at = candidate
            .head_created_ms
            .saturating_add(IDLE_VERIFY_DELAY_MS);
        if ready_at <= now {
            // 同一 Artwork 同时只跑一个校验任务：共享该作品的仓库文件与
            // 历史元数据，串行更可预测。
            if ready.len() < limit && artworks.insert(candidate.artwork_id.clone()) {
                ready.push(candidate.clone());
            }
        } else {
            next_ready_at =
                Some(next_ready_at.map_or(ready_at, |current: i64| current.min(ready_at)));
        }
    }
    IdleVerifySelection {
        next_ready_at: ready.is_empty().then_some(next_ready_at).flatten(),
        ready,
        blocked: false,
    }
}

/// 单个链路校验的只读结论。校验失败作为 `IdleVerifyError::Failed` 返回：
/// 它会被写入 `verify_error` 并使分支退出队列，与其他后台任务互不影响。
struct PreparedVerify {
    target: IdleVerifyTarget,
    outcome: Result<(), IdleVerifyError>,
}

#[derive(Debug)]
enum VerifyBatchError {
    ShuttingDown,
    Busy(String),
    Infrastructure(String),
}

impl From<String> for VerifyBatchError {
    fn from(error: String) -> Self {
        Self::Infrastructure(error)
    }
}

/// 并行准备 + 串行写回一批链路校验。
///
/// 文件校验是只读的，按 Artwork 最多并行 `IDLE_VERIFY_WORKERS` 条；写
/// `verified_history_id` / `verify_error` 仍经共享运行锁串行执行，不并行写事务。
fn run_verify_batch(
    app_state: &AppState,
    state: &BackupState,
    targets: &[IdleVerifyTarget],
) -> Result<(), VerifyBatchError> {
    if targets.is_empty() {
        return Ok(());
    }
    // 准备阶段只读且可能较长，因此不持有仓库租约；提交阶段先取共享运行锁，
    // 再按固定锁序进入仓库 lease 并再次确认仓库未切换。
    let Some(prepare_root) = app_state
        .repository_path()
        .map_err(VerifyBatchError::Infrastructure)?
    else {
        return Ok(());
    };
    // 内容级只读任务：只探测跨进程写者，不把校验变成写者。
    if let Err(error) = repository_lock::probe(&prepare_root) {
        return Err(VerifyBatchError::Busy(error));
    }
    let prepared = prepare_verify_batch(&prepare_root, state, targets);
    // 写回也需要跨进程 owner lock；它与上面的 `probe` 之间仍有竞态窗口，因此
    // 这里的「被占用」与 probe 一样按让位处理，而不是记一次错误。
    let owner = match acquire_owner(app_state, "backup:idleVerify") {
        Ok(owner) => owner,
        Err(error) if error == repository_lock::BUSY_MESSAGE => {
            return Err(VerifyBatchError::Busy(error));
        }
        Err(error) => return Err(VerifyBatchError::Infrastructure(error)),
    };
    let result = state.run_exclusive_typed(None, BackupTaskKind::IdleVerify, || {
        app_state
            .with_ready_repository(|root| {
                if root != prepare_root.as_path() {
                    // 准备期间仓库已被切换：本轮结果不属于当前仓库，整批丢弃。
                    return Ok(());
                }
                for item in &prepared {
                    if let Err(error) = commit_idle_verify(root, item) {
                        log::warn!(
                            "idle verify result could not be recorded for branch {}: {error}",
                            item.target.branch_id
                        );
                    }
                }
                Ok(())
            })
            .map_err(IdleVerifyError::Infrastructure)
    });
    drop(owner);
    match result {
        Ok(()) => Ok(()),
        Err(ExclusiveRunError::ShuttingDown) => Err(VerifyBatchError::ShuttingDown),
        Err(ExclusiveRunError::State(error)) => Err(VerifyBatchError::Infrastructure(error)),
        Err(ExclusiveRunError::Operation(IdleVerifyError::Infrastructure(error))) => {
            Err(VerifyBatchError::Infrastructure(error))
        }
        // 让位/取消只影响本轮，不写结果。
        Err(ExclusiveRunError::Operation(_)) => Ok(()),
    }
}

/// 并行读取并校验一批 head snapshot。线程只做文件与只读元数据工作，结果写回由
/// 调用方在共享运行锁内串行完成。
fn prepare_verify_batch(
    root: &Path,
    state: &BackupState,
    targets: &[IdleVerifyTarget],
) -> Vec<PreparedVerify> {
    let total = targets.len() as u64;
    let completed = Arc::new(AtomicUsize::new(0));
    state.report_progress("verify", "正在并行校验分支链路", 0, total);
    let mut prepared = Vec::with_capacity(targets.len());
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(targets.len());
        for target in targets {
            let state = state.clone();
            let completed = completed.clone();
            handles.push(scope.spawn(move || {
                // 只读探针：不占用共享运行锁，取消令牌只服务「后台让位」。
                let task =
                    state.begin_probe_task(BackupTaskKind::IdleVerify, Some(&target.branch_id));
                let outcome = prepare_idle_verify(root, &state, target, task.cancel_token());
                let done = completed.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                state.report_progress("verify", "正在并行校验分支链路", done as u64, total);
                outcome
            }));
        }
        for (target, handle) in targets.iter().cloned().zip(handles) {
            let outcome = handle.join().unwrap_or_else(|_| {
                Err(IdleVerifyError::Infrastructure("校验线程异常结束".into()))
            });
            prepared.push(PreparedVerify { target, outcome });
        }
    });
    prepared
}

/// 只读校验：复查候选与 head、读取 snapshot 并逐块校验，不写任何结果。
fn prepare_idle_verify(
    root: &Path,
    state: &BackupState,
    target: &IdleVerifyTarget,
    cancel: &CancelToken,
) -> Result<(), IdleVerifyError> {
    // 与到期自动备份相同的让位复查：手动提交优先、前台命令优先。
    if state.manual_pending(&target.branch_id) {
        return Err(IdleVerifyError::Deferred);
    }
    if state.should_yield_background() {
        return Err(IdleVerifyError::ForegroundWaiting);
    }
    if cancel.is_cancelled() {
        return Err(IdleVerifyError::Cancelled);
    }
    let current = history::load_idle_verify_target(root, &target.branch_id)
        .map_err(IdleVerifyError::Infrastructure)?
        .ok_or(IdleVerifyError::NotPending)?;
    if current.head_history_id != target.head_history_id {
        // 候选选择与校验开始之间 head 已经变化：本轮不写结果，留给下一次。
        return Err(IdleVerifyError::NotPending);
    }
    let record = history::load_node(root, &current.head_history_id)
        .map_err(IdleVerifyError::Infrastructure)?;
    let Some(relative) = record.snapshot_path.as_deref() else {
        return Err(IdleVerifyError::Failed(
            "分支 head 缺少 snapshot，无法校验".into(),
        ));
    };
    let path = storage::resolve_path(root, relative).map_err(IdleVerifyError::Infrastructure)?;
    let combined = CancelToken::with_source({
        let state = state.clone();
        let cancel = cancel.clone();
        move || state.should_yield_background() || cancel.is_cancelled()
    });
    match validate_snapshot(&path, &record, "链路校验", &combined) {
        Ok(()) => Ok(()),
        Err(error) => {
            if state.should_yield_background() {
                return Err(IdleVerifyError::ForegroundWaiting);
            }
            if cancel.is_cancelled() {
                return Err(IdleVerifyError::Cancelled);
            }
            Err(IdleVerifyError::Failed(error))
        }
    }
}

/// 在共享运行锁内写回一条校验结果。head 在准备期间变化时整条结果丢弃。
fn commit_idle_verify(root: &Path, prepared: &PreparedVerify) -> Result<(), IdleVerifyError> {
    let target = &prepared.target;
    let current = history::load_idle_verify_target(root, &target.branch_id)
        .map_err(IdleVerifyError::Infrastructure)?;
    let Some(current) = current else {
        return Ok(());
    };
    if current.head_history_id != target.head_history_id {
        return Ok(());
    }
    match &prepared.outcome {
        Ok(()) => {
            let verified_ms = storage::now_ms().map_err(IdleVerifyError::Infrastructure)?;
            history::mark_verified(
                root,
                &target.branch_id,
                &target.head_history_id,
                verified_ms,
            )
            .map_err(IdleVerifyError::Infrastructure)?;
            log::info!(
                "idle verify passed: branch_id={}, head={}",
                target.branch_id,
                target.head_history_id
            );
        }
        Err(IdleVerifyError::Failed(error)) => {
            history::mark_verify_error(root, &target.branch_id, &target.head_history_id, error)
                .map_err(IdleVerifyError::Infrastructure)?;
            log::warn!(
                "idle verify failed for branch {}: {error}",
                target.branch_id
            );
        }
        Err(IdleVerifyError::NotPending)
        | Err(IdleVerifyError::Deferred)
        | Err(IdleVerifyError::ForegroundWaiting)
        | Err(IdleVerifyError::Cancelled) => {}
        Err(IdleVerifyError::Infrastructure(error)) => {
            log::warn!(
                "idle verify infrastructure failure for branch {}: {error}",
                target.branch_id
            );
        }
    }
    Ok(())
}

/// 单条同步校验，供单元测试使用。
#[cfg(test)]
fn run_idle_verify(
    root: &Path,
    state: &BackupState,
    branch_id: &str,
    expected_head: &str,
) -> Result<(), IdleVerifyError> {
    let target = history::load_idle_verify_target(root, branch_id)
        .map_err(IdleVerifyError::Infrastructure)?
        .ok_or(IdleVerifyError::NotPending)?;
    if target.head_history_id != expected_head {
        return Err(IdleVerifyError::NotPending);
    }
    // 单条同步校验不登记任务，取消令牌保持独立；让位条件仍由
    // `prepare_idle_verify` 里的 `should_yield_background` 复查。
    let cancel = CancelToken::standalone();
    let outcome = prepare_idle_verify(root, state, &target, &cancel);
    let prepared = PreparedVerify { target, outcome };
    commit_idle_verify(root, &prepared)?;
    prepared.outcome
}

#[derive(Debug)]
enum AutomaticBackupError {
    NotScheduled,
    /// 分支工作文件位于网络位置或设备命名空间，本轮自动备份跳过且不计入失败。
    Skipped,
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
            Self::Skipped => formatter.write_str("分支工作文件不参与自动备份"),
            Self::Deferred => formatter.write_str("分支有待处理的手动提交，自动备份已延后"),
            Self::ForegroundWaiting => formatter.write_str("有前台命令在等待，自动备份已让位"),
            Self::Cancelled => formatter.write_str("自动备份已取消"),
            Self::Failed { error, .. } | Self::Infrastructure(error) => error.fmt(formatter),
        }
    }
}

#[derive(Debug, Clone)]
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
    options: ChunkWriteOptions,
) -> Result<BackupCommitResult, AutomaticBackupError> {
    let now = storage::now_ms().map_err(AutomaticBackupError::Infrastructure)?;
    let branch = history::load_scheduled(root, branch_id)
        .map_err(AutomaticBackupError::Infrastructure)?
        .ok_or(AutomaticBackupError::NotScheduled)?;
    // 无人值守流程的路径资格复查：候选筛选之后、取得运行锁之前分支工作文件可能被
    // 改成网络位置（或仓库由他人构造）。这里再判一次，命中就跳过且不计入失败——
    // 自动备份不应当因为一条路径资格判定而被关闭。
    if let Some(reason) = storage::unattended_source_path_rejection(&branch.source_path) {
        history::mark_automatic_backup_skipped(root, branch_id, &reason)
            .map_err(AutomaticBackupError::Infrastructure)?;
        return Err(AutomaticBackupError::Skipped);
    }
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
    // 快速检查：分支开启快速检查时，先只比较大小与修改时间；完全一致视为内容
    // 未变化，任何不一致都退回下方全量检查和备份流程。全局设置只在分支创建时
    // 提供初始值，不参与这里的判定。
    if branch.quick_enabled {
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
    let cancel = CancelToken::with_source({
        let state = state.clone();
        move || state.cancelled()
    });
    match worker::run_backup(root, branch_id, "", "automatic", &cancel, options) {
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

/// 调度器每次真正写仓库前取得与命令层相同的跨进程 owner lock。未配置仓库或目录
/// 尚未建立时返回 `None`，让候选查询自己给出规范错误。
fn acquire_owner(
    app_state: &AppState,
    operation: &str,
) -> Result<Option<RepositoryOwnerLock>, String> {
    let Some(root) = app_state.repository_path()? else {
        return Ok(None);
    };
    if !root.is_dir() {
        return Ok(None);
    }
    RepositoryOwnerLock::acquire(&root, app_state.process_role(), operation)
        .map(Some)
        .map_err(|error| match error {
            OwnerLockError::Busy(_) => crate::repository_lock::BUSY_MESSAGE.to_owned(),
            OwnerLockError::Io(message) => message,
        })
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
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source, true).unwrap();
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
        worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        make_due(&root, &branch_id);

        let result = run_scheduled_backup(
            &root,
            &BackupState::default(),
            &branch_id,
            ChunkWriteOptions::V1,
        )
        .unwrap();

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
        worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        make_due(&root, &branch_id);
        fs::write(&source, b"changed content").unwrap();

        let result = run_scheduled_backup(
            &root,
            &BackupState::default(),
            &branch_id,
            ChunkWriteOptions::V1,
        )
        .unwrap();

        assert!(result.created);
        assert_eq!(history::list(&root, &artwork_id).unwrap().nodes.len(), 2);
    }

    #[test]
    fn branch_quick_flag_uses_the_quick_path() {
        let (_directory, root, _source, _artwork_id, branch_id) = fixture();
        let first_id = worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
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

        let result = run_scheduled_backup(
            &root,
            &BackupState::default(),
            &branch_id,
            ChunkWriteOptions::V1,
        )
        .unwrap();

        assert!(!result.created);
        assert!(result.unchanged);
        assert_eq!(
            history::load_node(&root, &first_id).unwrap().snapshot_path,
            Some(original_relative)
        );
    }

    #[test]
    fn quick_disabled_branch_runs_full_check_even_when_content_is_unchanged() {
        let (_directory, root, _source, _artwork_id, branch_id) = fixture();
        let first_id = worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap()
        .history_id
        .unwrap();
        // 分支显式关闭快速检查：即使创建时的全局默认是快速，内容未变化也必须
        // 走全量路径核对并修复损坏的 head snapshot。
        history::update_branch(&root, &branch_id, "Main", true, true, 10, false, None).unwrap();
        let original_relative = history::load_node(&root, &first_id)
            .unwrap()
            .snapshot_path
            .unwrap();
        let original_path = crate::storage::resolve_path(&root, &original_relative).unwrap();
        let mut damaged = fs::read(&original_path).unwrap();
        *damaged.last_mut().unwrap() ^= 0xff;
        fs::write(&original_path, damaged).unwrap();
        make_due(&root, &branch_id);

        let result = run_scheduled_backup(
            &root,
            &BackupState::default(),
            &branch_id,
            ChunkWriteOptions::V1,
        )
        .unwrap();

        assert!(!result.created);
        assert!(result.unchanged);
        // 修复以新路径发布 head snapshot，与被破坏的原路径不同。
        assert_ne!(
            history::load_node(&root, &first_id).unwrap().snapshot_path,
            Some(original_relative)
        );
    }

    #[test]
    fn scheduled_backup_yields_to_a_waiting_foreground_command() {
        let (_directory, root, _source, artwork_id, branch_id) = fixture();
        worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        make_due(&root, &branch_id);
        let state = BackupState::default();
        // 模拟一个已登记、仍在等待运行锁的前台命令：后台任务取得运行锁后必须让位，
        // 且不计入失败、不推进检查时间。
        let waiting = state.begin_foreground_wait();

        let yielded = run_scheduled_backup(&root, &state, &branch_id, ChunkWriteOptions::V1);

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

        let result =
            run_scheduled_backup(&root, &state, &branch_id, ChunkWriteOptions::V1).unwrap();
        assert!(result.unchanged);
    }

    #[test]
    fn scheduled_backup_defers_to_a_pending_manual_request() {
        let (_directory, root, _source, _artwork_id, branch_id) = fixture();
        worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        make_due(&root, &branch_id);
        let state = BackupState::default();
        state.begin_manual_request(&branch_id);

        let deferred = run_scheduled_backup(&root, &state, &branch_id, ChunkWriteOptions::V1);

        assert!(matches!(deferred, Err(AutomaticBackupError::Deferred)));
        state.end_manual_request(&branch_id);
        let result =
            run_scheduled_backup(&root, &state, &branch_id, ChunkWriteOptions::V1).unwrap();
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
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source, true).unwrap();
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
        let result = run_scheduled_backup(
            &root,
            &BackupState::default(),
            &artwork.branch_id,
            ChunkWriteOptions::V1,
        );

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
            source_path: r"C:\work\artwork.bin".into(),
            last_check_ms: Some(10),
            interval_minutes: 120,
            quick_enabled: false,
            retry_at_ms: Some(42),
        };
        assert_eq!(due_at_ms(&branch, 1_000), 42);
    }

    #[test]
    fn scheduled_backup_skips_a_network_work_file_without_counting_a_failure() {
        let (_directory, root, _source, artwork_id, branch_id) = fixture();
        worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        make_due(&root, &branch_id);
        // 手工把工作文件改成 UNC 路径（等价于「打开他人构造的仓库」）。
        crate::storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE branches SET source_path = ?2, source_path_key = ?2 WHERE id = ?1",
                rusqlite::params![&branch_id, r"\\nas\share\artwork.bin"],
            )
            .unwrap();

        let result = run_scheduled_backup(
            &root,
            &BackupState::default(),
            &branch_id,
            ChunkWriteOptions::V1,
        );

        assert!(
            matches!(result, Err(AutomaticBackupError::Skipped)),
            "{result:?}"
        );
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("网络位置")));
        assert_eq!(branch.consecutive_backup_failures, 0);
        assert!(branch.backup_enabled);
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
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source, true).unwrap();
        let history_id = worker::run_backup(
            &root,
            &artwork.branch_id,
            "Initial",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
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

        let result = run_scheduled_backup(
            &root,
            &BackupState::default(),
            &artwork.branch_id,
            ChunkWriteOptions::V1,
        );

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
            crate::library::create_artwork(&root, None, "Artwork", "Main", &source, true).unwrap();
        let state = BackupState::default();
        let operation_state = state.clone();

        let result = state.run_exclusive_typed(
            Some(&artwork.branch_id),
            BackupTaskKind::AutomaticBackup,
            || {
                assert!(operation_state.request_cancel().unwrap());
                run_scheduled_backup(
                    &root,
                    &operation_state,
                    &artwork.branch_id,
                    ChunkWriteOptions::V1,
                )
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

    fn verify_target(artwork_id: &str, branch_id: &str, head_created_ms: i64) -> IdleVerifyTarget {
        IdleVerifyTarget {
            branch_id: branch_id.into(),
            artwork_id: artwork_id.into(),
            head_history_id: "head".into(),
            head_created_ms,
        }
    }

    #[test]
    fn idle_verify_selection_waits_for_the_head_to_settle() {
        let now = 1_000_000;
        let fresh = verify_target("artwork-a", "fresh", now - 1_000);
        let selection = select_idle_verify(&[fresh], now, false, 0, false, IDLE_VERIFY_WORKERS);
        assert!(selection.ready.is_empty());
        assert_eq!(
            selection.next_ready_at,
            Some(now - 1_000 + IDLE_VERIFY_DELAY_MS)
        );

        let settled = verify_target("artwork-a", "settled", now - IDLE_VERIFY_DELAY_MS - 1);
        let selection = select_idle_verify(&[settled], now, false, 0, false, IDLE_VERIFY_WORKERS);
        assert_eq!(selection.ready.len(), 1);
        assert_eq!(selection.ready[0].branch_id, "settled");
        assert!(selection.next_ready_at.is_none());
    }

    #[test]
    fn idle_verify_selection_yields_while_busy_or_paused() {
        let now = 1_000_000;
        let settled = verify_target("artwork-a", "settled", now - IDLE_VERIFY_DELAY_MS - 1);
        // 暂停所有自动备份时空闲校验一并暂停。
        let paused =
            select_idle_verify(&[settled.clone()], now, true, 0, false, IDLE_VERIFY_WORKERS);
        assert!(paused.blocked);
        assert!(paused.ready.is_empty());
        // 有前台命令在等待。
        let foreground = select_idle_verify(
            &[settled.clone()],
            now,
            false,
            1,
            false,
            IDLE_VERIFY_WORKERS,
        );
        assert!(foreground.blocked);
        // 存在待处理的手动提交。
        let manual = select_idle_verify(&[settled], now, false, 0, true, IDLE_VERIFY_WORKERS);
        assert!(manual.blocked);
    }

    #[test]
    fn idle_verify_selection_ignores_an_empty_queue() {
        let selection = select_idle_verify(&[], 1_000, false, 0, false, IDLE_VERIFY_WORKERS);
        assert!(selection.ready.is_empty());
        assert!(selection.next_ready_at.is_none());
        assert!(!selection.blocked);
    }

    #[test]
    fn idle_verify_selection_dedupes_by_artwork_and_respects_the_worker_limit() {
        let now = 1_000_000;
        let settled_at = now - IDLE_VERIFY_DELAY_MS - 1;
        let candidates = vec![
            verify_target("artwork-a", "a-main", settled_at),
            verify_target("artwork-a", "a-alt", settled_at),
            verify_target("artwork-b", "b-main", settled_at),
            verify_target("artwork-c", "c-main", settled_at),
        ];

        let all = select_idle_verify(&candidates, now, false, 0, false, IDLE_VERIFY_WORKERS);
        assert_eq!(all.ready.len(), 3);
        assert_eq!(
            all.ready
                .iter()
                .map(|target| target.branch_id.as_str())
                .collect::<Vec<_>>(),
            ["a-main", "b-main", "c-main"]
        );

        // 同一 Artwork 的第二个任务不会占用并行额度。
        let single = select_idle_verify(&candidates, now, false, 0, false, 1);
        assert_eq!(single.ready.len(), 1);
        assert_eq!(single.ready[0].branch_id, "a-main");
        assert!(single.next_ready_at.is_none());
    }

    #[test]
    fn verification_runs_before_backups_after_four_automatic_runs() {
        // 没有到期备份时，校验始终优先。
        assert!(verify_precedes_backup(false, true, 0));
        // 有到期备份时，连续 3 次自动备份后仍让备份先走。
        assert!(!verify_precedes_backup(true, true, 0));
        assert!(!verify_precedes_backup(
            true,
            true,
            IDLE_VERIFY_AGING_RUNS - 1
        ));
        // 第 4 次自动备份后插队一次校验，避免校验饥饿。
        assert!(verify_precedes_backup(true, true, IDLE_VERIFY_AGING_RUNS));
        // 没有可校验候选时 aging 不占用备份轮次。
        assert!(!verify_precedes_backup(true, false, IDLE_VERIFY_AGING_RUNS));
    }

    fn scheduled_branch(
        id: &str,
        last_check_ms: Option<i64>,
        retry_at_ms: Option<i64>,
    ) -> ScheduledBranch {
        ScheduledBranch {
            id: id.into(),
            source_path: r"C:\work\artwork.bin".into(),
            last_check_ms,
            interval_minutes: 60,
            quick_enabled: false,
            retry_at_ms,
        }
    }

    #[test]
    fn due_selection_orders_by_deadline_and_reports_the_next_one() {
        let now = 10_000_000;
        let state = BackupState::default();
        // last_check + 60min + jitter 都远大于 now：用 retry_at 直接给定截止时间，
        // 验证堆按最早到期出队，并平局时按上次检查时间与分支 ID 稳定排序。
        let branches = vec![
            scheduled_branch("later", Some(1), Some(now + 5_000)),
            scheduled_branch("first", Some(1), Some(now - 100)),
            scheduled_branch("tie-a", Some(2), Some(now + 1_000)),
            scheduled_branch("tie-b", Some(1), Some(now + 1_000)),
        ];

        let (due, next_due) = select_due_branch(branches, now, &state);
        assert_eq!(due.as_deref(), Some("first"));
        assert_eq!(next_due, Some(now - 100));

        // 下一次调用时已取出的分支不再出现；tie-b 的上次检查更早，先于 tie-a。
        let branches = vec![
            scheduled_branch("tie-a", Some(2), Some(now + 1_000)),
            scheduled_branch("tie-b", Some(1), Some(now + 1_000)),
        ];
        let (due, next_due) = select_due_branch(branches, now + 1_000, &state);
        assert_eq!(due.as_deref(), Some("tie-b"));
        assert_eq!(next_due, Some(now + 1_000));
    }

    #[test]
    fn a_failed_branch_backoff_does_not_block_another_due_branch() {
        let now = 10_000_000;
        let state = BackupState::default();
        // failed 分支的 retry_at 在未来（失败退避），不应参与本轮选择；
        // 另一个到期分支仍能被选中，下一次截止仍是 failed 的退避时间。
        let branches = vec![
            scheduled_branch("failed", Some(1), Some(now + 30_000)),
            scheduled_branch("healthy", Some(1), Some(now - 1)),
        ];
        let (due, next_due) = select_due_branch(branches, now, &state);
        assert_eq!(due.as_deref(), Some("healthy"));
        assert_eq!(next_due, Some(now - 1));

        let branches = vec![scheduled_branch("failed", Some(1), Some(now + 30_000))];
        let (due, next_due) = select_due_branch(branches, now, &state);
        assert!(due.is_none());
        assert_eq!(next_due, Some(now + 30_000));
    }

    #[test]
    fn a_manual_pending_branch_is_left_to_the_manual_request() {
        let now = 10_000_000;
        let state = BackupState::default();
        state.begin_manual_request("manual");
        let branches = vec![
            scheduled_branch("manual", Some(1), Some(now - 1)),
            scheduled_branch("automatic", Some(1), Some(now + 1_000)),
        ];

        let (due, next_due) = select_due_branch(branches, now, &state);
        state.end_manual_request("manual");
        assert!(due.is_none());
        assert_eq!(next_due, Some(now + 1_000));
    }

    #[test]
    fn background_tasks_keep_independent_cancel_tokens() {
        let state = BackupState::default();
        let first = state.begin_task(BackupTaskKind::IdleVerify, Some("a"));
        let second = state.begin_task(BackupTaskKind::IdleVerify, Some("b"));

        first.cancel_token().cancel();
        assert!(first.cancel_token().is_cancelled());
        assert!(!second.cancel_token().is_cancelled());
        assert!(state.cancelled());

        // 完成一个任务不会影响另一个任务的令牌与整体取消状态。
        first.complete();
        assert!(!state.cancelled());
        assert!(!second.cancel_token().is_cancelled());
        second.complete();
        assert!(!state.cancelled());
    }

    #[test]
    fn a_cancelled_verify_probe_does_not_cancel_the_foreground_command() {
        let state = BackupState::default();
        // 并行只读探针 + 刚取得运行锁的前台命令：前台进站时的让位请求只取消
        // 探针，不能让前台命令读到「已取消」。
        let probe = state.begin_probe_task(BackupTaskKind::IdleVerify, Some("a"));
        let user = state.begin_task(BackupTaskKind::UserOperation, None);

        assert!(state.cancel_background().unwrap());
        assert!(probe.cancel_token().is_cancelled());
        assert!(!state.cancelled());
        assert!(!user.cancel_token().is_cancelled());

        // 运行状态反映串行任务，而不是同时存在的空闲校验探针。
        let status = state.status().unwrap();
        assert_eq!(status.task_kind, Some(BackupTaskKind::UserOperation));

        // 探针逐个结束不得清掉前台命令已经上报的进度。
        state.report_progress("restore", "正在恢复历史", 3, 10);
        probe.complete();
        let status = state.status().unwrap();
        assert_eq!(status.task_kind, Some(BackupTaskKind::UserOperation));
        assert_eq!(status.progress_label.as_deref(), Some("正在恢复历史"));

        // 用户主动取消仍然作用于全部任务。
        assert!(state.request_cancel().unwrap());
        assert!(state.cancelled());
        user.complete();
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
        let history_id = worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
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
        let history_id = worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
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
        let first_id = worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap()
        .history_id
        .unwrap();
        history::mark_verify_error(&root, &branch_id, &first_id, "链路校验失败").unwrap();
        assert!(history::list_idle_verify_targets(&root).unwrap().is_empty());

        // 新提交产生新 head：分支重新入队，且旧失败摘要被清空。
        fs::write(&source, b"changed content").unwrap();
        let second_id = worker::run_backup(
            &root,
            &branch_id,
            "Second",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
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
        worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();

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
        let history_id = worker::run_backup(
            &root,
            &branch_id,
            "First",
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
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
