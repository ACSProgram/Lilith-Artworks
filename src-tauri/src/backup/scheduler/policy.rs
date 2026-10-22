//! 自动备份的调度策略：时间常量、代价感知的间隔、到期选择与快速检查阈值。
//!
//! 这里只做纯计算与候选排序，不取锁、不写库；执行与事件发送在 `scheduler` 主模块。

use std::{cmp::Ordering, collections::BinaryHeap, time::Duration};

use crate::{history::ScheduledBranch, storage};

use super::super::BackupState;

pub(super) const ERROR_RETRY: Duration = Duration::from_secs(60);
pub(super) const IDLE_RECHECK: Duration = Duration::from_secs(5 * 60);
/// 有前台命令在等待共享运行锁时的让位退避。它足够短以保证前台结束后的响应性，
/// 又足以避免调度器空转抢锁。
pub(super) const FOREGROUND_YIELD_BACKOFF: Duration = Duration::from_millis(200);
/// 用户长操作结束后，后台任务至少还要等待的静默时长。避免前台刚放锁、后台
/// 立刻插进操作间隙，随后又被下一次操作取消。
pub(super) const USER_ACTIVITY_SILENCE_MS: i64 = 10 * 1000;
/// 等待进程内写门释放的兜底超时。正常路径由闸门释放信号唤醒；超时只是防
/// 信号丢失，不让调度器变成 200 ms 轮询。
pub(super) const WRITE_GATE_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
/// 用户取消后台任务后的短暂退避：不把整支队列一起停 60 秒，也避免立刻重跑。
pub(super) const CANCEL_RETRY: Duration = Duration::from_secs(5);
/// 空闲完整性检查前，仓库最近一次历史写入至少需要静默的时长（10 分钟）。
/// 这是防抖而不是频率：连续提交会让窗口永不就绪，因此从 0.2 的「head 静默」
/// 放宽为「仓库静默」。
pub(super) const IDLE_VERIFY_SILENCE_MS: i64 = 10 * 60 * 1000;
/// 一个空闲窗口内最多推进的节点数，与时间预算先到为准。
pub(super) const IDLE_VERIFY_BATCH_NODES: usize = 8;
pub(super) const IDLE_VERIFY_BATCH_BUDGET: Duration = Duration::from_secs(30);
/// aging（防饥饿）：每连续完成 N 次自动备份后，若已有就绪的完整性检查，先让
/// 一批检查插队。维护者 2026-10-18 定稿为按次数、N = 4，不引入时钟规则。
pub(super) const IDLE_VERIFY_AGING_RUNS: u32 = 4;
/// 快速命中计数的基准阈值：空闲的小仓库每 8 次检查做一次内容确认
/// （第 8 次本身是全量，中间最多放过 7 次指纹）。
pub(super) const AUDIT_BASE_HITS: u32 = 8;
/// 全量确认代价的分档基线。256 MiB 以下一次全量通常在一秒内，超过后按档
/// 推迟；分档上限为 4，避免大文件在忙时把阈值抬得过高。
pub(super) const AUDIT_SIZE_BASE_BYTES: i64 = 256 * 1024 * 1024;
/// 无论后台多忙，每条分支大约每七天必须完整读一次内容。它按当前有效间隔
/// 折算成检查次数，不追踪墙钟时间，因此只是近似上限。
pub(super) const AUDIT_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

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

/// 从候选集合中取出最早到期的一条；同时给出下一次到期时刻与本轮到期分支数。
pub(super) fn select_due_branch(
    branches: Vec<ScheduledBranch>,
    now: i64,
    state: &BackupState,
) -> (Option<String>, Option<i64>, usize) {
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
    let due_count = heap.iter().filter(|entry| entry.due_at <= now).count();
    let next_due = heap.peek().map(|entry| entry.due_at);
    let due = if heap.peek().is_some_and(|entry| entry.due_at <= now) {
        heap.pop().map(|entry| entry.branch.id)
    } else {
        None
    };
    (due, next_due, due_count)
}

/// 第二优先级是否先于本轮自动备份执行：没有到期备份时始终优先；有到期备份时
/// 仅在连续 `IDLE_VERIFY_AGING_RUNS` 次自动备份后插队一次，避免校验被持续
/// 到期的备份饿死。
pub(super) fn verify_precedes_backup(
    due: bool,
    verify_ready: bool,
    automatic_since_verify: u32,
) -> bool {
    verify_ready && (!due || automatic_since_verify >= IDLE_VERIFY_AGING_RUNS)
}

/// 实际检查间隔取 `max(用户设置的间隔, 2 × 上次耗时)`：一次检查比间隔还长时，
/// 等它跑完再等同样长的时间，避免连续满负荷。耗时优先取本进程刚记下的值，
/// 没有时回退到库里的上次成功检查，因此重启后第一轮不会丢掉代价感知。
pub(super) fn effective_interval_ms(branch: &ScheduledBranch, state: &BackupState) -> i64 {
    let nominal = i64::from(branch.interval_minutes).saturating_mul(60_000);
    let last_elapsed_ms = state
        .check_elapsed(&branch.id)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .or(branch.last_check_elapsed_ms);
    last_elapsed_ms
        .map(|elapsed| nominal.max(elapsed.saturating_mul(2)))
        .unwrap_or(nominal)
}

/// 下一次自动备份的截止时刻。
pub(super) fn due_at_ms(branch: &ScheduledBranch, now: i64, state: &BackupState) -> i64 {
    branch.retry_at_ms.unwrap_or_else(|| {
        branch.last_check_ms.map_or(now, |checked| {
            checked
                .saturating_add(effective_interval_ms(branch, state))
                .saturating_add(jitter_ms(&branch.id))
        })
    })
}

/// 本轮到期队列的推迟档位：0–7 条视为正常，8 条起每翻倍加一档。
pub(super) fn audit_queue_factor(due_branches: usize) -> u32 {
    match due_branches {
        0..=7 => 1,
        8..=15 => 2,
        16..=31 => 3,
        32..=63 => 4,
        64..=127 => 5,
        128..=255 => 6,
        256..=511 => 7,
        _ => 8,
    }
}

/// 全量确认代价的档位：按上次全量时的工作文件大小分档，未知按最小档。
pub(super) fn audit_size_factor(last_source_size: Option<i64>) -> u32 {
    let size = last_source_size.unwrap_or(0).max(0);
    if size <= AUDIT_SIZE_BASE_BYTES {
        1
    } else if size <= AUDIT_SIZE_BASE_BYTES * 2 {
        2
    } else if size <= AUDIT_SIZE_BASE_BYTES * 4 {
        3
    } else {
        4
    }
}

/// 两次全量内容确认之间最多允许的检查次数。当前连续命中数 + 1 达到该值
/// 时，本次检查跳过指纹直接做全量。
///
/// 三个因子分别对应：空闲时保持最小确认频率、忙时把额外的全量推迟到队列
/// 消化之后、大文件减少全量次数；最后用七天年龄上限兜底。检查间隔很长时
/// 年龄上限会低于基准值，允许阈值小于 8——此时「七天一次」优先于最小次数。
pub(super) fn audit_threshold(
    branch: &ScheduledBranch,
    due_branches: usize,
    state: &BackupState,
) -> u32 {
    let target = AUDIT_BASE_HITS
        .saturating_mul(audit_queue_factor(due_branches))
        .saturating_mul(audit_size_factor(branch.last_source_size));
    let interval_ms = effective_interval_ms(branch, state).max(1);
    let age_cap = u32::try_from((AUDIT_MAX_AGE_MS / interval_ms).max(1)).unwrap_or(u32::MAX);
    target.min(age_cap).max(1)
}

pub(super) fn jitter_ms(branch_id: &str) -> i64 {
    let hash = branch_id.bytes().fold(0_u64, |value, byte| {
        value.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    ((hash % 21) as i64 - 10) * 1_000
}
