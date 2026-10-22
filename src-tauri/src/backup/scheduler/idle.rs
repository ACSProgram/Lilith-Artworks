//! 空闲完整性校验：窗口判定、时机门控、批次执行与让位条件。
//!
//! 调度主循环只负责选择第二优先级并处理批次结果；本模块不发送完成事件。

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use crate::{app::AppState, cancel::CancelToken, history, repository_lock, storage};

use super::super::{runtime::ExclusiveRunError, verify_history_node, BackupState, BackupTaskKind};
use super::policy::{
    select_due_branch, IDLE_VERIFY_BATCH_BUDGET, IDLE_VERIFY_BATCH_NODES, IDLE_VERIFY_SILENCE_MS,
};
use super::{acquire_owner, OwnerAttempt};

/// 空闲窗口的进程内判定：主窗口从「可见且未最小化」转为「隐藏或最小化」时
/// 开启一个新窗口；每个窗口最多自动执行一批完整性检查。无持久化、无计时器。
#[derive(Default)]
pub(super) struct IdleWindowTracker {
    idle: bool,
    generation: u64,
    consumed: Option<u64>,
}

impl IdleWindowTracker {
    /// 返回当前窗口代次与该代次是否已经尝试过一批。
    pub(super) fn observe(&mut self, idle: bool) -> (u64, bool) {
        if idle && !self.idle {
            self.generation = self.generation.wrapping_add(1);
        }
        self.idle = idle;
        (
            self.generation,
            idle && self.consumed == Some(self.generation),
        )
    }

    pub(super) fn mark_batch_attempted(&mut self, generation: u64) {
        self.consumed = Some(generation);
    }
}

/// 空闲完整性检查的第二优先级选择结果。
#[derive(Default)]
pub(super) struct IdleVerifySelection {
    /// 仓库已静默满 `IDLE_VERIFY_SILENCE_MS`，可以立即跑一批。要校验哪些节点由
    /// `run_verify_batch_locked` 在锁内重新读取，锁外只判断「有没有」。
    pub(super) ready: bool,
    /// 仓库最接近静默满足的时刻；已就绪或 `blocked` 时为 `None`。
    pub(super) next_ready_at: Option<i64>,
    /// 第二优先级整体让位（暂停、窗口不空闲、窗口名额已用、前台等待或手动提交）。
    pub(super) blocked: bool,
}

/// 空闲完整性检查的时机门控输入。
#[derive(Clone, Copy)]
pub(super) struct IdleVerifyGate {
    pub(super) paused: bool,
    pub(super) window_idle: bool,
    pub(super) window_consumed: bool,
    pub(super) foreground_waiting: usize,
    pub(super) manual_pending: bool,
}

/// 第二优先级选择：窗口必须隐藏或最小化、本窗口尚未尝试过、仓库已静默至少
/// `IDLE_VERIFY_SILENCE_MS`，且没有暂停、前台等待或待处理手动提交。
pub(super) fn select_idle_verify(
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
pub(super) enum VerifyBatchError {
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
pub(super) fn run_verify_batch(
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
    let mut blocked = 0_u64;
    let mut writer = history::VerificationWriter::open(root)?;
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
        match verify_history_node(root, &target.history_id, &cancel) {
            Ok(()) => {
                if state.should_yield_background() || state.cancelled() {
                    log::debug!(
                        "idle integrity batch preempted before recording: history_id={}",
                        target.history_id
                    );
                    break;
                }
                let verified_ms = storage::now_ms()?;
                writer.record_verified(&target.history_id, verified_ms)?;
                verified += 1;
            }
            Err(failure) => {
                if cancel.is_cancelled() || state.cancelled() {
                    log::debug!(
                        "idle integrity batch preempted during node: history_id={}",
                        target.history_id
                    );
                    break;
                }
                // 真正坏掉的可能是更深的节点：只有它算「校验失败」，被它挡住的
                // 那些节点写入「无法检查」，不再重复记成同一个故障。
                let message = failure.message_for(&target.history_id);
                writer.record_error(&target.history_id, &message)?;
                if failure.blames(&target.history_id) {
                    failed += 1;
                    log::warn!(
                        "idle integrity check failed: history_id={}, error={message}",
                        target.history_id
                    );
                } else {
                    blocked += 1;
                    log::warn!(
                        "idle integrity check could not verify: history_id={}, reason={message}",
                        target.history_id
                    );
                }
            }
        }
        checked += 1;
        state.report_progress("verify", "正在校验历史节点", checked, total);
        if started.elapsed() >= IDLE_VERIFY_BATCH_BUDGET {
            log::debug!("idle integrity batch reached the time budget");
            break;
        }
    }
    // 正常结束（含被前台抢占而提前停止）提交已完成的节点；本批最多 8 个，
    // 未走满批量阈值时由这里统一落库。
    writer.finish()?;
    log::info!(
        "idle integrity batch finished: checked={checked}, verified={verified}, failed={failed}, blocked={blocked}, elapsed_ms={}",
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
