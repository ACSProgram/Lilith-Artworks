use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use tauri::{AppHandle, Manager};

use crate::{
    app::{
        command_runtime::{
            AppEvent, AppEventKind, CommandError, CommandErrorSummary, OperationDomain,
            OperationOutcome, SharedEventSink,
        },
        AppState,
    },
    cancel::CancelToken,
    history::{self, ScheduledBranch},
    platform::path::display_path,
    repository_lock::{self, OwnerLockError, RepositoryOwnerLock, WaitPolicy, WritePriority},
    storage,
};

use super::chunk_file::ChunkWriteOptions;

use super::{runtime::ExclusiveRunError, worker, BackupCommitResult, BackupState, BackupTaskKind};

const ERROR_RETRY: Duration = Duration::from_secs(60);
const IDLE_RECHECK: Duration = Duration::from_secs(5 * 60);
/// 有前台命令在等待共享运行锁时的让位退避。它足够短以保证前台结束后的响应性，
/// 又足以避免调度器空转抢锁。
const FOREGROUND_YIELD_BACKOFF: Duration = Duration::from_millis(200);
/// 用户长操作结束后，后台任务至少还要等待的静默时长。避免前台刚放锁、后台
/// 立刻插进操作间隙，随后又被下一次操作取消。
const USER_ACTIVITY_SILENCE_MS: i64 = 10 * 1000;
/// 等待进程内写门释放的兜底超时。正常路径由闸门释放信号唤醒；超时只是防
/// 信号丢失，不让调度器变成 200 ms 轮询。
const WRITE_GATE_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
/// 用户取消后台任务后的短暂退避：不把整支队列一起停 60 秒，也避免立刻重跑。
const CANCEL_RETRY: Duration = Duration::from_secs(5);
/// 空闲完整性检查前，仓库最近一次历史写入至少需要静默的时长（10 分钟）。
/// 这是防抖而不是频率：连续提交会让窗口永不就绪，因此从 0.2 的「head 静默」
/// 放宽为「仓库静默」。
const IDLE_VERIFY_SILENCE_MS: i64 = 10 * 60 * 1000;
/// 一个空闲窗口内最多推进的节点数，与时间预算先到为准。
const IDLE_VERIFY_BATCH_NODES: usize = 8;
const IDLE_VERIFY_BATCH_BUDGET: Duration = Duration::from_secs(30);
/// aging（防饥饿）：每连续完成 N 次自动备份后，若已有就绪的完整性检查，先让
/// 一批检查插队。维护者 2026-10-18 定稿为按次数、N = 4，不引入时钟规则。
const IDLE_VERIFY_AGING_RUNS: u32 = 4;

pub(crate) fn run(state: BackupState, app: AppHandle, events: SharedEventSink) {
    let mut automatic_since_verify = 0_u32;
    let mut idle_window = IdleWindowTracker::default();
    loop {
        if !state.wait_scheduler(Duration::ZERO) {
            break;
        }
        let app_state = app.state::<AppState>().inner().clone();
        // 托盘"暂停所有自动备份"同时暂停空闲完整性检查：用户意图是"别动仓库"。
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
        let now = match storage::now_ms() {
            Ok(now) => now,
            Err(_) => {
                if !state.wait_scheduler(ERROR_RETRY) {
                    break;
                }
                continue;
            }
        };
        // 用户操作结束后先静默一段时间再开后台任务。用户操作**不**唤醒调度器
        // （素材板之类的频繁保存不该反复触发候选扫描），这里只按剩余静默期挂起，
        // 醒来后重新计算剩余时长；连续操作期间一轮一轮往后推，不会空转。
        if let Some(wait) = user_activity_wait(&app_state, now) {
            if !state.wait_scheduler(wait) {
                break;
            }
            continue;
        }
        let idle_integrity_enabled = app_state.idle_integrity_check_enabled();
        let window_idle = idle_integrity_enabled && state.window_idle();
        let (window_generation, window_consumed) = idle_window.observe(window_idle);
        // 候选筛选会顺带写「自动备份被跳过」的原因，因此同样按一次写操作取得
        // 跨进程 owner lock。
        let candidates_owner = match acquire_owner(&app_state, "backup:candidates") {
            Ok(OwnerAttempt::Owned(owner)) => owner,
            // 本进程有前台/交互写者：让位，不写分支失败、不推进退避。
            Ok(OwnerAttempt::Skipped(root)) => {
                log::debug!("scheduler yields the candidate scan to an in-process writer");
                if !wait_for_write_gate(&state, &root) {
                    break;
                }
                continue;
            }
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
        // 完成事件需要把分支映射回作品；候选已经带有 artwork_id，这里只留一份
        // 轻量映射，不额外查库。
        let branch_artworks = branches
            .iter()
            .map(|branch| (branch.id.clone(), branch.artwork_id.clone()))
            .collect::<HashMap<_, _>>();
        // deadline 优先：候选按 due_at、上次检查时间与分支 ID 排序；失败退避已
        // 写在分支自己的 retry_at 里，因此一个分支失败不会阻塞其它到期分支。
        let (due, next_due) = select_due_branch(branches, now, &state);
        let selection = if idle_integrity_enabled {
            // 锁外只判断「有没有待查节点」与仓库静默时长；具体校验哪些节点由
            // `run_verify_batch_locked` 在锁内重新读取。
            let candidates = app_state.with_repository_read(|root| {
                Ok((
                    !history::list_pending_verify_nodes(root, 1)?.is_empty(),
                    history::latest_history_write_ms(root)?,
                ))
            });
            match candidates {
                Ok((pending, latest_write_ms)) => select_idle_verify(
                    pending,
                    latest_write_ms,
                    now,
                    IdleVerifyGate {
                        paused,
                        window_idle,
                        window_consumed,
                        foreground_waiting: state.foreground_waiting(),
                        manual_pending: state.any_manual_pending(),
                    },
                ),
                Err(_) => {
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                    continue;
                }
            }
        } else {
            IdleVerifySelection::default()
        };
        let verify_ready = selection.ready;
        if verify_precedes_backup(due.is_some(), verify_ready, automatic_since_verify) {
            // 有到期备份却仍插队（aging）时，本批不再把「备份已到期」当作中途让位
            // 条件——否则 aging 永远走不完第一批；新到达的前台与手动提交仍然让位。
            let had_due_backup = due.is_some();
            let outcome = run_verify_batch(&app_state, &state, had_due_backup);
            // 一次尝试无论完成、失败还是被抢占，都消耗该空闲窗口的名额。
            idle_window.mark_batch_attempted(window_generation);
            match outcome {
                Ok(()) => {
                    automatic_since_verify = 0;
                    // 校验只改写节点/分支的校验状态，不改变历史图结构。
                    emit_background_completion(
                        &events,
                        &app_state,
                        &state,
                        None,
                        false,
                        OperationOutcome::Succeeded,
                        None,
                    );
                }
                Err(VerifyBatchError::ShuttingDown) => break,
                Err(VerifyBatchError::InProcessWriter(root)) => {
                    log::debug!("idle integrity check yielded to an in-process writer");
                    if !wait_for_write_gate(&state, &root) {
                        break;
                    }
                }
                Err(VerifyBatchError::CrossProcessBusy(error)) => {
                    // 跨进程占用等不到本进程信号，直接短退避，不空转抢锁。
                    log::debug!("idle integrity check yielded to another process: {error}");
                    if !state.wait_scheduler(FOREGROUND_YIELD_BACKOFF) {
                        break;
                    }
                }
                Err(VerifyBatchError::Infrastructure(error)) => {
                    log::error!("idle integrity batch failed: {error}");
                    if !state.wait_scheduler(ERROR_RETRY) {
                        break;
                    }
                }
            }
            continue;
        }

        if let Some(branch_id) = due {
            let backup_owner = match acquire_owner(&app_state, "backup:automatic") {
                Ok(OwnerAttempt::Owned(owner)) => owner,
                // 本进程有前台/交互写者：本轮让位，不写失败也不推进退避。
                Ok(OwnerAttempt::Skipped(root)) => {
                    log::debug!("scheduler yields branch {branch_id} to an in-process writer");
                    if !wait_for_write_gate(&state, &root) {
                        break;
                    }
                    continue;
                }
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
            let artwork_id = branch_artworks.get(&branch_id).cloned();
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
                                app_state.last_user_activity_ms(),
                                app_state.chunk_write_options(),
                            ))
                        })
                        .map_err(AutomaticBackupError::Infrastructure)?
                },
            );
            drop(backup_owner);
            match result {
                // aging 只统计真正完成的自动备份；让位、跳过与失败都不推进计数。
                Ok(backup) => {
                    automatic_since_verify = automatic_since_verify.saturating_add(1);
                    emit_background_completion(
                        &events,
                        &app_state,
                        &state,
                        artwork_id.as_deref(),
                        backup.created,
                        OperationOutcome::Succeeded,
                        None,
                    );
                }
                Err(ExclusiveRunError::Operation(
                    AutomaticBackupError::NotScheduled
                    | AutomaticBackupError::Skipped
                    | AutomaticBackupError::Deferred
                    | AutomaticBackupError::ForegroundWaiting
                    | AutomaticBackupError::UserActive,
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
                    emit_background_completion(
                        &events,
                        &app_state,
                        &state,
                        artwork_id.as_deref(),
                        false,
                        OperationOutcome::Failed,
                        Some(CommandError::from_legacy(error).summary()),
                    );
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
            // 让位期间由唤醒信号驱动恢复（手动提交结束、窗口状态变化都会唤醒），
            // 不必空转抢锁。
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

/// 空闲窗口的进程内判定：主窗口从「可见且未最小化」转为「隐藏或最小化」时
/// 开启一个新窗口；每个窗口最多自动执行一批完整性检查。无持久化、无计时器。
#[derive(Default)]
struct IdleWindowTracker {
    idle: bool,
    generation: u64,
    consumed: Option<u64>,
}

impl IdleWindowTracker {
    /// 返回当前窗口代次与该代次是否已经尝试过一批。
    fn observe(&mut self, idle: bool) -> (u64, bool) {
        if idle && !self.idle {
            self.generation = self.generation.wrapping_add(1);
        }
        self.idle = idle;
        (
            self.generation,
            idle && self.consumed == Some(self.generation),
        )
    }

    fn mark_batch_attempted(&mut self, generation: u64) {
        self.consumed = Some(generation);
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
        let due_at = due_at_ms(&branch, now, state);
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

/// 空闲完整性检查的第二优先级选择结果。
#[derive(Default)]
struct IdleVerifySelection {
    /// 仓库已静默满 `IDLE_VERIFY_SILENCE_MS`，可以立即跑一批。要校验哪些节点由
    /// `run_verify_batch_locked` 在锁内重新读取，锁外只判断「有没有」。
    ready: bool,
    /// 仓库最接近静默满足的时刻；已就绪或 `blocked` 时为 `None`。
    next_ready_at: Option<i64>,
    /// 第二优先级整体让位（暂停、窗口不空闲、窗口名额已用、前台等待或手动提交）。
    blocked: bool,
}

/// 空闲完整性检查的时机门控输入。
#[derive(Clone, Copy)]
struct IdleVerifyGate {
    paused: bool,
    window_idle: bool,
    window_consumed: bool,
    foreground_waiting: usize,
    manual_pending: bool,
}

/// 第二优先级选择：窗口必须隐藏或最小化、本窗口尚未尝试过、仓库已静默至少
/// `IDLE_VERIFY_SILENCE_MS`，且没有暂停、前台等待或待处理手动提交。
fn select_idle_verify(
    pending: bool,
    latest_write_ms: Option<i64>,
    now: i64,
    gate: IdleVerifyGate,
) -> IdleVerifySelection {
    if !pending {
        return IdleVerifySelection::default();
    }
    if gate.paused
        || !gate.window_idle
        || gate.window_consumed
        || gate.foreground_waiting > 0
        || gate.manual_pending
    {
        return IdleVerifySelection {
            ready: false,
            next_ready_at: None,
            blocked: true,
        };
    }
    let ready_at = latest_write_ms.map(|written| written.saturating_add(IDLE_VERIFY_SILENCE_MS));
    if let Some(ready_at) = ready_at.filter(|at| *at > now) {
        return IdleVerifySelection {
            ready: false,
            next_ready_at: Some(ready_at),
            blocked: false,
        };
    }
    IdleVerifySelection {
        ready: true,
        next_ready_at: None,
        blocked: false,
    }
}

#[derive(Debug)]
enum VerifyBatchError {
    ShuttingDown,
    /// 本进程有前台/交互写者占住写入闸门：等那扇闸门的释放信号，不轮询。
    InProcessWriter(PathBuf),
    /// 其它进程占用仓库：本进程没有可等的信号，只能短退避后重试。
    CrossProcessBusy(String),
    Infrastructure(String),
}

/// 在空闲窗口内执行一批节点级完整性检查。
///
/// 校验是只读的，但结果写入节点级校验列，因此整批持有共享运行锁与跨进程
/// owner lock；每个节点边界都重新检查让位条件。`had_due_backup` 表示本批是
/// aging 插队（有到期备份仍然放行），此时不再把「备份已到期」当作中途让位条件。
fn run_verify_batch(
    app_state: &AppState,
    state: &BackupState,
    had_due_backup: bool,
) -> Result<(), VerifyBatchError> {
    let Some(prepare_root) = app_state
        .repository_path()
        .map_err(VerifyBatchError::Infrastructure)?
    else {
        return Ok(());
    };
    if !prepare_root.is_dir() {
        return Ok(());
    }
    let owner = match acquire_owner(app_state, "backup:idleVerify") {
        Ok(OwnerAttempt::Owned(owner)) => owner,
        // 本进程有前台/交互写者：整批让位，本窗口名额照常消耗。
        Ok(OwnerAttempt::Skipped(root)) => return Err(VerifyBatchError::InProcessWriter(root)),
        Err(error) if error == repository_lock::BUSY_MESSAGE => {
            return Err(VerifyBatchError::CrossProcessBusy(error));
        }
        Err(error) => return Err(VerifyBatchError::Infrastructure(error)),
    };
    let result = state.run_exclusive_typed(None, BackupTaskKind::IdleVerify, || {
        app_state.with_ready_repository(|root| {
            if root != prepare_root.as_path() {
                // 准备期间仓库已被切换：本批不属于当前仓库，直接丢弃。
                return Ok(());
            }
            run_verify_batch_locked(root, app_state, state, had_due_backup)
        })
    });
    drop(owner);
    match result {
        Ok(()) => Ok(()),
        Err(ExclusiveRunError::ShuttingDown) => Err(VerifyBatchError::ShuttingDown),
        Err(ExclusiveRunError::State(error)) => Err(VerifyBatchError::Infrastructure(error)),
        Err(ExclusiveRunError::Operation(error)) => Err(VerifyBatchError::Infrastructure(error)),
    }
}

fn run_verify_batch_locked(
    root: &Path,
    app_state: &AppState,
    state: &BackupState,
    had_due_backup: bool,
) -> Result<(), String> {
    if !verify_batch_gate_open(root, app_state, state, had_due_backup)? {
        return Ok(());
    }
    // 锁内重新读取待查集合：候选选择与取锁之间可能有提交或手动重查。
    let targets = history::list_pending_verify_nodes(root, IDLE_VERIFY_BATCH_NODES)?;
    if targets.is_empty() {
        return Ok(());
    }
    let total = targets.len() as u64;
    let started = Instant::now();
    let mut checked = 0_u64;
    let mut verified = 0_u64;
    let mut failed = 0_u64;
    state.report_progress("verify", "正在校验历史节点", 0, total);
    log::info!("idle integrity batch started: nodes={total}");
    for target in &targets {
        if !verify_batch_gate_open(root, app_state, state, had_due_backup)? {
            log::debug!(
                "idle integrity batch preempted at node boundary: history_id={}",
                target.history_id
            );
            break;
        }
        let cancel = CancelToken::with_source({
            let state = state.clone();
            move || state.should_yield_background() || state.cancelled()
        });
        match super::verify_history_node(root, &target.history_id, &cancel) {
            Ok(()) => {
                if state.should_yield_background() || state.cancelled() {
                    log::debug!(
                        "idle integrity batch preempted before recording: history_id={}",
                        target.history_id
                    );
                    break;
                }
                let verified_ms = storage::now_ms()?;
                history::mark_node_verified(root, &target.history_id, verified_ms)?;
                verified += 1;
            }
            Err(error) => {
                if cancel.is_cancelled() || state.cancelled() {
                    log::debug!(
                        "idle integrity batch preempted during node: history_id={}",
                        target.history_id
                    );
                    break;
                }
                history::mark_node_verify_error(root, &target.history_id, &error)?;
                failed += 1;
                log::warn!(
                    "idle integrity check failed: history_id={}, error={error}",
                    target.history_id
                );
            }
        }
        checked += 1;
        state.report_progress("verify", "正在校验历史节点", checked, total);
        if started.elapsed() >= IDLE_VERIFY_BATCH_BUDGET {
            log::debug!("idle integrity batch reached the time budget");
            break;
        }
    }
    log::info!(
        "idle integrity batch finished: checked={checked}, verified={verified}, failed={failed}, elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(())
}

/// 批次开始与每个节点边界共用的门控复查：设置仍开启、窗口仍空闲、没有前台
/// 等待/手动提交/取消，仓库仍静默；非 aging 批次还要确认没有到期自动备份。
fn verify_batch_gate_open(
    root: &Path,
    app_state: &AppState,
    state: &BackupState,
    had_due_backup: bool,
) -> Result<bool, String> {
    if !app_state.idle_integrity_check_enabled()
        || !state.window_idle()
        || state.should_yield_background()
        || state.cancelled()
    {
        return Ok(false);
    }
    let now = storage::now_ms()?;
    if let Some(latest) = history::latest_history_write_ms(root)? {
        if latest.saturating_add(IDLE_VERIFY_SILENCE_MS) > now {
            return Ok(false);
        }
    }
    if !had_due_backup && automatic_backup_due(root, state, now)? {
        return Ok(false);
    }
    Ok(true)
}

fn automatic_backup_due(root: &Path, state: &BackupState, now: i64) -> Result<bool, String> {
    let branches = history::list_scheduled(root)?;
    Ok(select_due_branch(branches, now, state).0.is_some())
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
    /// 用户长操作刚结束，静默窗口未满；本轮让位，不计入失败。
    UserActive,
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
            Self::UserActive => formatter.write_str("用户刚操作完仓库，自动备份已延后"),
            Self::Cancelled => formatter.write_str("自动备份已取消"),
            Self::Failed { error, .. } | Self::Infrastructure(error) => error.fmt(formatter),
        }
    }
}

fn run_scheduled_backup(
    root: &Path,
    state: &BackupState,
    branch_id: &str,
    last_user_activity_ms: Option<i64>,
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
    if due_at_ms(&branch, now, state) > now {
        return Err(AutomaticBackupError::NotScheduled);
    }
    // 取锁之后复查用户静默：候选选择与加锁之间用户可能刚完成一次操作。
    // 让位不计入失败，也不推进检查时间。
    if last_user_activity_ms.is_some_and(|last| now.saturating_sub(last) < USER_ACTIVITY_SILENCE_MS)
    {
        return Err(AutomaticBackupError::UserActive);
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
    let checked_started = Instant::now();
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
            state.record_check_elapsed(branch_id, checked_started.elapsed());
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
        Ok(result) => {
            state.record_check_elapsed(branch_id, checked_started.elapsed());
            Ok(result)
        }
        Err(worker::BackupRunError::Cancelled) => Err(AutomaticBackupError::Cancelled),
        Err(worker::BackupRunError::Failed(error)) => {
            let failed_ms = storage::now_ms().map_err(AutomaticBackupError::Infrastructure)?;
            let disabled = history::mark_automatic_backup_error(root, branch_id, &error, failed_ms)
                .map_err(AutomaticBackupError::Infrastructure)?;
            Err(AutomaticBackupError::Failed { error, disabled })
        }
    }
}

/// 发一条后台任务完成事件。`structure_changed` 让前端区分「历史图结构变了」
/// 与「只改了状态行」：后者不必重建 mindmap，也不必作废缩略图缓存。
///
/// `completion_revision` 在任务结束之后读取，正常就是本次任务推进出的值。若期间
/// 恰好还有别的任务完成，事件会带上更新的 revision，前端因为找不到中间 revision
/// 的事件而退回完整刷新——不会误判成「只改了状态行」。
fn emit_background_completion(
    events: &SharedEventSink,
    app_state: &AppState,
    state: &BackupState,
    artwork_id: Option<&str>,
    structure_changed: bool,
    outcome: OperationOutcome,
    error: Option<CommandErrorSummary>,
) {
    let repository_key = app_state
        .repository_path()
        .ok()
        .flatten()
        .map(|path| display_path(&path));
    let revision = state
        .status()
        .map(|status| status.completion_revision)
        .unwrap_or(0);
    let mut event = AppEvent::new(
        AppEventKind::OperationFinished {
            outcome,
            error,
            artwork_id: artwork_id.map(str::to_owned),
            structure_changed: Some(structure_changed),
        },
        repository_key,
        None,
        Some(OperationDomain::Backup),
        false,
        revision,
    );
    event.phase = Some("backgroundComplete".to_owned());
    events.emit(&event);
}

/// 用户长操作是否仍在冷却期；返回还需要等待的时长。
fn user_activity_wait(app_state: &AppState, now: i64) -> Option<Duration> {
    let last = app_state.last_user_activity_ms()?;
    let ready_at = last.saturating_add(USER_ACTIVITY_SILENCE_MS);
    (ready_at > now).then(|| Duration::from_millis(u64::try_from(ready_at - now).unwrap_or(0)))
}

/// 挂起等待进程内写门释放，返回 `false` 表示应用正在退出。超时后照常返回，
/// 由调用方重新检查闸门。
fn wait_for_write_gate(state: &BackupState, root: &Path) -> bool {
    let cancel = CancelToken::with_source({
        let state = state.clone();
        move || state.shutting_down()
    });
    repository_lock::wait_for_write_gate_release(root, WRITE_GATE_RELEASE_TIMEOUT, &cancel);
    state.wait_scheduler(Duration::ZERO)
}

/// 下一次自动备份的截止时刻。实际间隔取 `max(用户设置的间隔, 2 × 上次耗时)`：
/// 一次检查比间隔还长时，等它跑完再等同样长的时间，避免连续满负荷。学习结果
/// 只在内存里，重启后第一轮按名义间隔跑。
fn due_at_ms(branch: &ScheduledBranch, now: i64, state: &BackupState) -> i64 {
    branch.retry_at_ms.unwrap_or_else(|| {
        branch.last_check_ms.map_or(now, |checked| {
            let nominal = i64::from(branch.interval_minutes) * 60_000;
            let effective = state
                .check_elapsed(&branch.id)
                .map(|elapsed| {
                    let measured =
                        i64::try_from(elapsed.as_millis().saturating_mul(2)).unwrap_or(i64::MAX);
                    nominal.max(measured)
                })
                .unwrap_or(nominal);
            checked
                .saturating_add(effective)
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

/// 调度器一次取锁的结果。
///
/// `Skipped` 表示**本进程**已有前台/交互写者占用写入闸门：后台任务永不排队，
/// 本轮直接让位，既不算失败也不推进分支退避。真实跨进程冲突仍走 `Err`。
enum OwnerAttempt {
    /// `Box` 压小枚举（`RepositoryOwnerLock` 内含连接与线程句柄），锁本身不
    /// 因此改变语义。
    Owned(Option<Box<RepositoryOwnerLock>>),
    /// 本进程有前台/交互写者占位。携带仓库根目录，供等待写门释放使用。
    Skipped(PathBuf),
}

/// 调度器每次真正写仓库前取得与命令层相同的跨进程 owner lock。未配置仓库或目录
/// 尚未建立时返回 `None`，让候选查询自己给出规范错误。
///
/// 后台优先级 + `Try` 策略：进程内有写者时立即 [`OwnerAttempt::Skipped`]；
/// 另一个进程持锁时仍原样返回 [`repository_lock::BUSY_MESSAGE`]。
fn acquire_owner(app_state: &AppState, operation: &str) -> Result<OwnerAttempt, String> {
    let Some(root) = app_state.repository_path()? else {
        return Ok(OwnerAttempt::Owned(None));
    };
    if !root.is_dir() {
        return Ok(OwnerAttempt::Owned(None));
    }
    match RepositoryOwnerLock::acquire_queued(
        &root,
        app_state.process_role(),
        operation,
        WritePriority::Background,
        WaitPolicy::Try,
        &CancelToken::standalone(),
    ) {
        Ok(lock) => Ok(OwnerAttempt::Owned(Some(Box::new(lock)))),
        Err(OwnerLockError::InProcessBusy) => Ok(OwnerAttempt::Skipped(root)),
        Err(OwnerLockError::Busy(_)) => Err(repository_lock::BUSY_MESSAGE.to_owned()),
        Err(error) => Err(error.user_message()),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, path::Path};

    use super::*;
    use crate::{
        app::settings::AppSettings,
        repository_lock::{acquire_write_gate, WaitPolicy, WritePriority},
    };

    /// 构造一个指向 fixture 仓库的 AppState，供调度器取锁用例使用。
    fn app_state_for(root: &Path, directory: &Path) -> AppState {
        let mut settings = AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();
        AppState::new(
            settings,
            directory.join("settings.json"),
            directory.join("logs"),
            None,
        )
    }

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
            None,
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
            None,
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
            None,
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
            None,
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

        let yielded = run_scheduled_backup(&root, &state, &branch_id, None, ChunkWriteOptions::V1);

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
            run_scheduled_backup(&root, &state, &branch_id, None, ChunkWriteOptions::V1).unwrap();
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

        let deferred = run_scheduled_backup(&root, &state, &branch_id, None, ChunkWriteOptions::V1);

        assert!(matches!(deferred, Err(AutomaticBackupError::Deferred)));
        state.end_manual_request(&branch_id);
        let result =
            run_scheduled_backup(&root, &state, &branch_id, None, ChunkWriteOptions::V1).unwrap();
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
            None,
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
    fn due_time_widens_the_interval_to_twice_the_last_check() {
        let state = BackupState::default();
        let branch = scheduled_branch("slow", Some(1_000_000), None);
        let nominal = 1_000_000 + 60 * 60 * 1_000 + jitter_ms("slow");
        assert_eq!(due_at_ms(&branch, 0, &state), nominal);

        // 上次检查耗时 2 小时：实际间隔放宽到 4 小时，用户设置的 60 分钟不变。
        state.record_check_elapsed("slow", Duration::from_secs(2 * 60 * 60));
        let widened = due_at_ms(&branch, 0, &state);
        assert_eq!(widened, 1_000_000 + 4 * 60 * 60 * 1_000 + jitter_ms("slow"));
        assert!(widened > nominal);
    }

    #[test]
    fn user_activity_wait_covers_the_silence_window() {
        let (directory, root, _source, _artwork_id, _branch_id) = fixture();
        let app_state = app_state_for(&root, directory.path());
        assert_eq!(user_activity_wait(&app_state, 1_000), None);

        app_state.note_user_activity();
        let now = storage::now_ms().unwrap();
        let wait = user_activity_wait(&app_state, now).expect("刚写过应仍在冷却期");
        assert!(wait <= Duration::from_millis(USER_ACTIVITY_SILENCE_MS as u64));
        assert_eq!(
            user_activity_wait(&app_state, now + USER_ACTIVITY_SILENCE_MS),
            None
        );
    }

    #[test]
    fn scheduled_backup_defers_while_the_user_just_wrote() {
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
        let now = storage::now_ms().unwrap();

        let deferred =
            run_scheduled_backup(&root, &state, &branch_id, Some(now), ChunkWriteOptions::V1);
        assert!(matches!(deferred, Err(AutomaticBackupError::UserActive)));

        // 让位不算失败，也不推进检查时间。
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.consecutive_backup_failures, 0);
        assert!(branch.last_error.is_none());
        let last_check: i64 = crate::storage::open(&root)
            .unwrap()
            .query_row(
                "SELECT last_check_ms FROM branches WHERE id = ?1",
                [&branch_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(last_check, 1);

        // 静默满 10 秒后正常检查，并记下本次耗时供调度器放宽间隔。
        let result = run_scheduled_backup(
            &root,
            &state,
            &branch_id,
            Some(now - USER_ACTIVITY_SILENCE_MS),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        assert!(result.unchanged);
        assert!(state.check_elapsed(&branch_id).is_some());
    }

    #[derive(Default)]
    struct RecordingSink {
        events: std::sync::Mutex<Vec<AppEvent>>,
    }

    impl crate::app::command_runtime::EventSink for RecordingSink {
        fn emit(&self, event: &AppEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn background_completion_event_carries_artwork_and_structure_flag() {
        let (directory, root, _source, artwork_id, _branch_id) = fixture();
        let app_state = app_state_for(&root, directory.path());
        let state = BackupState::default();
        state
            .run_exclusive(None, BackupTaskKind::UserOperation, || Ok::<_, String>(()))
            .unwrap();
        let sink = std::sync::Arc::new(RecordingSink::default());
        let events: SharedEventSink = sink.clone();

        emit_background_completion(
            &events,
            &app_state,
            &state,
            Some(&artwork_id),
            true,
            OperationOutcome::Succeeded,
            None,
        );

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.domain, Some(OperationDomain::Backup));
        assert_eq!(event.completion_revision, 1);
        assert_eq!(
            event.repository_key.as_deref(),
            Some(display_path(&root).as_str())
        );
        match &event.kind {
            AppEventKind::OperationFinished {
                outcome: OperationOutcome::Succeeded,
                artwork_id: Some(id),
                structure_changed: Some(true),
                ..
            } => assert_eq!(id, &artwork_id),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn due_time_uses_retry_deadline_without_interval_jitter() {
        let branch = ScheduledBranch {
            id: "branch".into(),
            artwork_id: "artwork".into(),
            source_path: r"C:\work\artwork.bin".into(),
            last_check_ms: Some(10),
            interval_minutes: 120,
            quick_enabled: false,
            retry_at_ms: Some(42),
        };
        assert_eq!(due_at_ms(&branch, 1_000, &BackupState::default()), 42);
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
            None,
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
            None,
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
                    None,
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

    fn open_idle_gate() -> IdleVerifyGate {
        IdleVerifyGate {
            paused: false,
            window_idle: true,
            window_consumed: false,
            foreground_waiting: 0,
            manual_pending: false,
        }
    }

    /// 待查节点数。断言用它而不是 `list_pending_verify_nodes(..).len()`：后者带批次
    /// 上限，只用于「按最旧优先取一批」的场景。
    fn pending_verify_count(root: &Path) -> usize {
        history::count_pending_verify_nodes(root).unwrap()
    }

    #[test]
    fn idle_verify_selection_waits_for_repository_silence() {
        let now = 1_000_000;
        let selection = select_idle_verify(true, Some(now - 1_000), now, open_idle_gate());
        assert!(!selection.ready);
        assert_eq!(
            selection.next_ready_at,
            Some(now - 1_000 + IDLE_VERIFY_SILENCE_MS)
        );

        let selection = select_idle_verify(
            true,
            Some(now - IDLE_VERIFY_SILENCE_MS - 1),
            now,
            open_idle_gate(),
        );
        assert!(selection.ready);
        assert!(selection.next_ready_at.is_none());
    }

    #[test]
    fn idle_verify_selection_requires_an_unused_idle_window() {
        let now = 1_000_000;
        // 窗口可见、未最小化。
        let visible = select_idle_verify(
            true,
            None,
            now,
            IdleVerifyGate {
                window_idle: false,
                ..open_idle_gate()
            },
        );
        assert!(visible.blocked);
        assert!(!visible.ready);
        // 本窗口已经用掉一批。
        let consumed = select_idle_verify(
            true,
            None,
            now,
            IdleVerifyGate {
                window_consumed: true,
                ..open_idle_gate()
            },
        );
        assert!(consumed.blocked);
        // 暂停所有自动备份。
        let paused = select_idle_verify(
            true,
            None,
            now,
            IdleVerifyGate {
                paused: true,
                ..open_idle_gate()
            },
        );
        assert!(paused.blocked);
        // 有前台命令在等待。
        let foreground = select_idle_verify(
            true,
            None,
            now,
            IdleVerifyGate {
                foreground_waiting: 1,
                ..open_idle_gate()
            },
        );
        assert!(foreground.blocked);
        // 存在待处理的手动提交。
        let manual = select_idle_verify(
            true,
            None,
            now,
            IdleVerifyGate {
                manual_pending: true,
                ..open_idle_gate()
            },
        );
        assert!(manual.blocked);
    }

    #[test]
    fn idle_verify_selection_ignores_an_empty_queue() {
        let selection = select_idle_verify(false, None, 1_000, open_idle_gate());
        assert!(!selection.ready);
        assert!(selection.next_ready_at.is_none());
        assert!(!selection.blocked);
    }

    /// 队列顺序由 SQL 决定：最久未读（`created_ms` 最小）的节点先校验。
    #[test]
    fn pending_verify_nodes_are_listed_oldest_first() {
        let (_directory, root, source, _artwork_id, branch_id) = fixture();
        let first_id = commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        let second_id = commit_node(&root, &branch_id, "Second");

        let pending = history::list_pending_verify_nodes(&root, IDLE_VERIFY_BATCH_NODES).unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|target| target.history_id.as_str())
                .collect::<Vec<_>>(),
            [first_id.as_str(), second_id.as_str()]
        );
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
            artwork_id: format!("artwork-{id}"),
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
    fn a_branch_without_head_is_not_queued_for_verification() {
        let (_directory, root, _source, _artwork_id, _branch_id) = fixture();
        // 已建分支但尚未提交：没有任何历史节点，派生队列必须为空。
        assert_eq!(pending_verify_count(&root), 0);
    }

    fn app_state_for_idle_verify(root: &Path, directory: &Path) -> AppState {
        let mut settings = AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();
        settings.idle_integrity_check_enabled = true;
        AppState::new(
            settings,
            directory.join("settings.json"),
            directory.join("logs"),
            None,
        )
    }

    fn commit_node(root: &Path, branch_id: &str, title: &str) -> String {
        worker::run_backup(
            root,
            branch_id,
            title,
            "manual",
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap()
        .history_id
        .unwrap()
    }

    /// 把历史写入时间推到 20 分钟前：满足「仓库静默 10 分钟」，同时把分支间隔
    /// 放宽到 60 分钟，避免静默本身让自动备份变成到期。
    fn make_repository_silent(root: &Path) {
        let old = crate::storage::now_ms().unwrap() - 20 * 60 * 1000;
        let connection = crate::storage::open(root).unwrap();
        connection
            .execute("UPDATE history_nodes SET created_ms = ?1", [old])
            .unwrap();
        connection
            .execute(
                "UPDATE branches SET last_check_ms = ?1, last_success_ms = ?1,
                    backup_interval_minutes = 60",
                [old],
            )
            .unwrap();
    }

    fn run_idle_batch(
        root: &Path,
        directory: &Path,
        state: &BackupState,
        had_due_backup: bool,
    ) -> Result<(), VerifyBatchError> {
        let app_state = app_state_for_idle_verify(root, directory);
        run_verify_batch(&app_state, state, had_due_backup)
    }

    #[test]
    fn idle_batch_marks_healthy_nodes_verified() {
        let (directory, root, source, artwork_id, branch_id) = fixture();
        commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        commit_node(&root, &branch_id, "Second");
        make_repository_silent(&root);
        assert_eq!(pending_verify_count(&root), 2);
        assert_eq!(history::count_pending_verify_nodes(&root).unwrap(), 2);

        let state = BackupState::default();
        state.set_window_idle(true);
        run_idle_batch(&root, directory.path(), &state, false).unwrap();

        assert_eq!(pending_verify_count(&root), 0);
        assert_eq!(history::count_pending_verify_nodes(&root).unwrap(), 0);
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch.verify_error.is_none());
        assert!(branch.verified_ms.is_some());
    }

    #[test]
    fn idle_batch_records_a_failure_on_the_offending_node_and_keeps_going() {
        let (directory, root, source, artwork_id, branch_id) = fixture();
        let first_id = commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        let second_id = commit_node(&root, &branch_id, "Second");
        // 破坏 head snapshot：两个节点都沿这条快照物化，都会失败。
        let record = history::load_node(&root, &second_id).unwrap();
        let path = storage::resolve_path(&root, record.snapshot_path.as_deref().unwrap()).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&path, bytes).unwrap();
        make_repository_silent(&root);

        let state = BackupState::default();
        state.set_window_idle(true);
        run_idle_batch(&root, directory.path(), &state, false).unwrap();

        // 失败不阻断批次，也不自动重试：两个节点都退出派生队列并各自记录错误。
        assert_eq!(pending_verify_count(&root), 0);
        let connection = crate::storage::open(&root).unwrap();
        let mut statement = connection
            .prepare("SELECT id, verify_error FROM history_nodes ORDER BY created_ms")
            .unwrap();
        let errors = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(errors.len(), 2);
        // 只有 head 的新快照块损坏；父节点通过 delta 重建时不会读到这个坏块，
        // 因此它仍通过校验——失败恰好定位到携带损坏内容的节点。
        let error_for = |id: &str| {
            errors
                .iter()
                .find(|(node_id, _)| node_id == id)
                .and_then(|(_, error)| error.as_deref())
        };
        assert!(error_for(&first_id).is_none());
        assert!(error_for(&second_id).is_some_and(|error| error.contains(&second_id)));
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch.verify_error.is_some());
        assert!(branch.verified_ms.is_none());
    }

    #[test]
    fn idle_batch_yields_without_writing_while_foreground_work_waits() {
        let (directory, root, _source, artwork_id, branch_id) = fixture();
        commit_node(&root, &branch_id, "First");
        make_repository_silent(&root);
        let state = BackupState::default();
        state.set_window_idle(true);

        let waiting = state.begin_foreground_wait();
        run_idle_batch(&root, directory.path(), &state, false).unwrap();
        drop(waiting);

        assert_eq!(pending_verify_count(&root), 1);
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch.verified_ms.is_none());
    }

    #[test]
    fn idle_batch_requires_an_enabled_setting_and_an_idle_window() {
        let (directory, root, source, _artwork_id, branch_id) = fixture();
        commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        commit_node(&root, &branch_id, "Second");
        make_repository_silent(&root);
        let state = BackupState::default();

        // 窗口可见（默认）时绝不自动运行。
        state.set_window_idle(false);
        run_idle_batch(&root, directory.path(), &state, false).unwrap();
        assert_eq!(pending_verify_count(&root), 2);

        // 窗口空闲但设置关闭时同样不运行。
        let app_state = app_state_for(&root, directory.path());
        state.set_window_idle(true);
        run_verify_batch(&app_state, &state, false).unwrap();
        assert_eq!(pending_verify_count(&root), 2);
    }

    #[test]
    fn idle_batch_defers_to_a_due_automatic_backup_unless_aging_allows_it() {
        let (directory, root, _source, _artwork_id, branch_id) = fixture();
        commit_node(&root, &branch_id, "First");
        make_repository_silent(&root);
        make_due(&root, &branch_id);
        let state = BackupState::default();
        state.set_window_idle(true);

        run_idle_batch(&root, directory.path(), &state, false).unwrap();
        assert_eq!(pending_verify_count(&root), 1);

        // aging 插队：即使有到期备份也允许跑完一批。
        run_idle_batch(&root, directory.path(), &state, true).unwrap();
        assert_eq!(pending_verify_count(&root), 0);
    }

    #[test]
    fn branch_status_derives_failure_and_oldest_verification_from_the_chain() {
        let (_directory, root, source, artwork_id, branch_id) = fixture();
        let first_id = commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        let second_id = commit_node(&root, &branch_id, "Second");

        history::mark_node_verified(&root, &first_id, 1_000).unwrap();
        history::mark_node_verified(&root, &second_id, 2_000).unwrap();
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.verified_ms, Some(1_000));
        assert!(branch.verify_error.is_none());

        // 链上任一节点失败 = 分支失败；失败节点同时失去 verified_ms。
        history::mark_node_verify_error(&root, &first_id, "损坏").unwrap();
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.verify_error.as_deref(), Some("损坏"));
        assert!(branch.verified_ms.is_none());
    }

    #[test]
    fn chain_rewrites_requeue_every_node_of_the_artwork() {
        let (directory, root, _source, _artwork_id, branch_id) = fixture();
        let first_id = commit_node(&root, &branch_id, "First");
        make_repository_silent(&root);
        let state = BackupState::default();
        state.set_window_idle(true);
        run_idle_batch(&root, directory.path(), &state, false).unwrap();
        assert_eq!(pending_verify_count(&root), 0);

        // 建立检查点会改写链的物化语义，按作品粗粒度清空校验状态。
        history::mark_checkpoint(&root, &first_id).unwrap();
        assert_eq!(pending_verify_count(&root), 1);
    }

    #[test]
    fn repository_scrub_refreshes_every_node_status() {
        let (_directory, root, source, artwork_id, branch_id) = fixture();
        commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        commit_node(&root, &branch_id, "Second");
        assert_eq!(pending_verify_count(&root), 2);

        let scanned =
            crate::backup::scrub_history(&root, &CancelToken::standalone(), |_, _| {}).unwrap();
        assert_eq!(scanned, 2);
        assert_eq!(pending_verify_count(&root), 0);
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert!(branch.verified_ms.is_some());
        assert!(branch.verify_error.is_none());
    }

    #[test]
    fn repository_scrub_continues_after_a_corrupt_node() {
        let (_directory, root, source, _artwork_id, branch_id) = fixture();
        let first_id = commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        let second_id = commit_node(&root, &branch_id, "Second");
        let record = history::load_node(&root, &second_id).unwrap();
        let path = storage::resolve_path(&root, record.snapshot_path.as_deref().unwrap()).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&path, bytes).unwrap();

        let error =
            crate::backup::scrub_history(&root, &CancelToken::standalone(), |_, _| {}).unwrap_err();
        assert!(error.contains(&second_id), "{error}");

        // 失败节点之后仍会继续扫描，并重写每个节点的状态。
        assert_eq!(pending_verify_count(&root), 0);
        assert!(history::load_node(&root, &first_id).is_ok());
        let errors = crate::storage::open(&root)
            .unwrap()
            .prepare("SELECT id, verify_error, verified_ms FROM history_nodes")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let first = errors.iter().find(|(id, _, _)| id == &first_id).unwrap();
        let second = errors.iter().find(|(id, _, _)| id == &second_id).unwrap();
        assert!(first.1.is_none() && first.2.is_some());
        assert!(second.1.is_some() && second.2.is_none());
    }

    #[test]
    fn manual_reverify_helpers_cover_the_whole_branch_chain() {
        let (_directory, root, source, _artwork_id, branch_id) = fixture();
        let first_id = commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        let second_id = commit_node(&root, &branch_id, "Second");

        assert_eq!(
            history::branch_chain_ids(&root, &branch_id).unwrap(),
            [first_id.clone(), second_id.clone()]
        );

        history::mark_node_verify_error(&root, &first_id, "旧错误").unwrap();
        history::mark_node_verify_error(&root, &second_id, "旧错误").unwrap();
        assert_eq!(pending_verify_count(&root), 0);

        history::clear_branch_verify_errors(&root, &branch_id).unwrap();
        assert_eq!(pending_verify_count(&root), 2);
    }

    #[test]
    fn snapshot_creation_requeues_every_node_of_the_artwork() {
        let (directory, root, source, _artwork_id, branch_id) = fixture();
        let first_id = commit_node(&root, &branch_id, "First");
        fs::write(&source, b"changed content").unwrap();
        commit_node(&root, &branch_id, "Second");
        make_repository_silent(&root);
        let state = BackupState::default();
        state.set_window_idle(true);
        run_idle_batch(&root, directory.path(), &state, false).unwrap();
        assert_eq!(pending_verify_count(&root), 0);

        crate::backup::ensure_checkpoint(
            &root,
            &first_id,
            &CancelToken::standalone(),
            ChunkWriteOptions::V1,
        )
        .unwrap();
        assert_eq!(pending_verify_count(&root), 2);
    }

    #[test]
    fn idle_window_tracker_allows_one_batch_per_idle_period() {
        let mut tracker = IdleWindowTracker::default();
        assert_eq!(tracker.observe(false), (0, false));
        let (generation, consumed) = tracker.observe(true);
        assert!(!consumed);
        tracker.mark_batch_attempted(generation);
        assert_eq!(tracker.observe(true), (generation, true));

        // 窗口重新可见再隐藏才开启新窗口。
        assert_eq!(tracker.observe(false), (generation, false));
        let (next_generation, consumed) = tracker.observe(true);
        assert_ne!(next_generation, generation);
        assert!(!consumed);
    }

    /// 后台任务不排队：本进程的写者持有写入闸门时，本轮取锁必须得到 `Skipped`
    /// （让位退避），而不是一次「跨进程占用」失败，分支记录也不得被写坏。
    #[test]
    fn the_scheduler_skips_the_round_while_an_in_process_writer_holds_the_gate() {
        let (directory, root, _source, artwork_id, _branch_id) = fixture();
        let app_state = app_state_for(&root, directory.path());

        // 本进程的前台/交互写者占住闸门（不取跨进程锁，模拟正写到一半）。
        // 持锁必须在另一个线程：闸门刻意不可重入，同线程再取会被断言拦下。
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder_root = root.clone();
        let holder = std::thread::spawn(move || {
            let guard = acquire_write_gate(
                &holder_root,
                WritePriority::Interactive,
                WaitPolicy::Try,
                &CancelToken::standalone(),
            )
            .unwrap();
            acquired_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            drop(guard);
        });
        acquired_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("持有闸门的线程未启动");

        assert!(
            matches!(
                acquire_owner(&app_state, "backup:automatic"),
                Ok(OwnerAttempt::Skipped(_))
            ),
            "进程内写者占位时后台必须跳过，而不是等锁或报错"
        );

        // 跳过不等于失败：分支记录保持干净，退避也不推进。
        let branch = history::list(&root, &artwork_id)
            .unwrap()
            .branches
            .remove(0);
        assert_eq!(branch.consecutive_backup_failures, 0);
        assert!(branch.last_error.is_none());

        release_tx.send(()).unwrap();
        holder.join().unwrap();
        // 闸门空闲后后台重新可以取锁。
        match acquire_owner(&app_state, "backup:automatic") {
            Ok(OwnerAttempt::Owned(Some(_))) => {}
            other => panic!("闸门空闲后后台应能取锁（is_ok={}）", other.is_ok()),
        }
    }
}
