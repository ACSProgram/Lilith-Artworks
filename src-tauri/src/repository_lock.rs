//! 跨进程仓库 owner lock。
//!
//! 单实例插件只覆盖 GUI 入口；无头命令、维护命令与 GUI 仍可能同时写同一个仓库。
//! 这里用仓库根目录下的专用 SQLite 数据库做**跨进程独占标记**：`BEGIN IMMEDIATE`
//! 的写锁由 SQLite 持有，进程退出或崩溃时由操作系统关闭句柄、锁自动释放。它不
//! 替代 SQLite 的 WAL + `busy_timeout`（那只保证数据库层不损坏），而是把业务层
//! 「不支持同仓库多进程写入」这条边界钉死。
//!
//! **存活判定**：锁本身是唯一的存活证明。取得 `BEGIN IMMEDIATE` 即说明旧 owner
//! 已退出或崩溃，其元数据一律视为 stale 并直接覆盖；绝不根据 PID 或心跳强行回收
//! ——Windows 的 PID 会被复用，且 SQLite 写锁无法被安全地外部打断。owner 元数据
//! （PID、启动时间、版本、角色、操作、心跳）写在同目录的 JSON 侧车文件里，只用于
//! 冲突提示与事后诊断。
//!
//! 覆盖范围：GUI、headless 与维护入口**每次写操作**在进入仓库锁序前 acquire，
//! 操作结束释放；只读命令不取锁。全库校验（`scrub`）只探测锁是否被占用，并在
//! 写者持锁时明确拒绝。
//!
//! **两层语义**：跨进程 owner lock 冲突仍然**立即拒绝、不排队**（「不支持同仓库
//! 多进程写入」是产品边界）；同一进程内的写者则在它之前的**进程内写入闸门**
//! （[`acquire_write_gate`]）里排队，按 Shutdown > Foreground > Interactive >
//! Background 的优先级串行，等待有上限且可取消。这样同进程的两条写路径不再互相
//! 冒充「另一个进程」。

use std::{
    cell::RefCell,
    cmp::Ordering as CmpOrdering,
    collections::{BinaryHeap, HashMap},
    fs,
    path::{Path, PathBuf},
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex, MutexGuard, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::cancel::CancelToken;

/// 专用锁数据库的仓库内相对名。点号开头，避免出现在用户可见的文件列表里。
pub(crate) const LOCK_FILE_NAME: &str = ".lilith-write-lock.sqlite3";
/// owner 元数据侧车文件。它不是锁；锁始终是上面的 SQLite 数据库。
pub(crate) const OWNER_FILE_NAME: &str = ".lilith-write-owner.json";
/// 写者持锁时第二个写进程得到的稳定文案（与 0.2 的哨兵保持一致）。
pub(crate) const BUSY_MESSAGE: &str = "仓库正在被另一个进程写入；不支持同一仓库的多进程写入";
/// 进程内排队超时或本进程写者占位时的文案：与「另一个进程写入」明确区分，避免同
/// 进程竞争时把用户指向一个并不存在的问题。
pub(crate) const WRITE_WAIT_TIMEOUT_MESSAGE: &str = "仓库正在写入，请稍后重试";
/// 内容级只读命令在写者持锁时的拒绝文案。
pub(crate) const CONTENT_READ_BUSY_MESSAGE: &str = "仓库正在被写入，请稍后重试";
/// 心跳间隔。仅用于诊断元数据；锁的存活不依赖它。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
/// 进程内排队的最短轮询间隔：等待者每隔这么久复查一次取消位与截止时间，不必完全
/// 依赖通知时序，也让「持锁线程被挂起」时仍能在上限内退出。
const GATE_POLL_INTERVAL: Duration = Duration::from_millis(50);

static PROCESS_STARTED_MS: OnceLock<i64> = OnceLock::new();

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 在进程启动的最早时机记录墙钟时间，供 owner 元数据使用。未调用时回退为首次
/// 取得锁的时间（单测与嵌入场景）。
pub(crate) fn record_process_start() {
    let _ = PROCESS_STARTED_MS.set(now_ms());
}

fn process_started_ms() -> i64 {
    *PROCESS_STARTED_MS.get_or_init(now_ms)
}

/// 当前或最近一次写操作的 owner 元数据。字段只用于诊断与冲突提示。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryOwner {
    pub(crate) pid: u32,
    pub(crate) process_started_ms: i64,
    pub(crate) app_version: String,
    pub(crate) role: String,
    pub(crate) operation: String,
    pub(crate) run_id: String,
    pub(crate) heartbeat_ms: i64,
}

impl RepositoryOwner {
    fn describe(&self) -> String {
        format!(
            "pid={}, process_started_ms={}, version={}, role={}, operation={}, run_id={}, heartbeat_ms={}",
            self.pid,
            self.process_started_ms,
            self.app_version,
            self.role,
            self.operation,
            self.run_id,
            self.heartbeat_ms
        )
    }
}

#[derive(Debug)]
pub(crate) enum OwnerLockError {
    /// 另一个进程正持锁；附带可读到的 owner 元数据（通常存在）。
    /// 元数据只用于测试断言与诊断，生产错误映射只需要稳定的用户文案。
    #[allow(dead_code)] // 生产代码只解构 `_`；owner 元数据由测试读取。
    Busy(Option<RepositoryOwner>),
    /// 本进程的另一个写者正持有写入闸门（`WaitPolicy::Try`）。
    InProcessBusy,
    /// 进程内排队到截止时刻仍未取得闸门。
    TimedOut,
    /// 排队期间取消令牌被置位。
    Cancelled,
    Io(String),
}

impl OwnerLockError {
    pub(crate) fn user_message(&self) -> String {
        match self {
            Self::Busy(_) => BUSY_MESSAGE.to_owned(),
            Self::InProcessBusy | Self::TimedOut => WRITE_WAIT_TIMEOUT_MESSAGE.to_owned(),
            Self::Cancelled => "操作已取消".to_owned(),
            Self::Io(message) => message.clone(),
        }
    }
}

/// 进程内写入优先级。声明顺序即优先级顺序（越小越紧急），闸门队列按它排序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WritePriority {
    /// 退出结算：插队到最前，保证关窗时素材板不会因为自动备份而结算失败。
    Shutdown,
    /// 前台长命令：恢复、精简、检查点、整仓备份、发布、全库扫描、切换仓库。
    Foreground,
    /// 普通写命令、素材板写命令、清理队列重放。
    Interactive,
    /// 自动备份与空闲校验：只 try，永不排队。
    Background,
}

/// 进程内排队策略。
#[derive(Debug, Clone, Copy)]
pub(crate) enum WaitPolicy {
    /// 只尝试一次：拿不到立即返回 [`GateError::Busy`]，不登记等待者。
    Try,
    /// 排队到指定时刻；超时返回 [`GateError::TimedOut`]。
    Until(Instant),
}

/// 进程内写入闸门的失败分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateError {
    /// 闸门正被本进程的另一个写者持有（或已有等待者），`Try` 策略立即返回。
    Busy,
    /// 排队到截止时刻仍未取得闸门。
    TimedOut,
    /// 排队期间取消令牌被置位；等待者已出队，不会再被后续释放唤醒。
    Cancelled,
    /// 同线程重复获取。闸门刻意非重入：重入会等自己，必然死锁。
    Reentrant,
}

/// 排队中的写者。`Ord` 反向实现：`BinaryHeap::peek` 返回最紧急的一位
/// （优先级最高；同优先级按登记顺序先到先得）。
#[derive(Debug, Clone, Copy)]
struct Waiting {
    priority: WritePriority,
    ticket: u64,
}

impl PartialEq for Waiting {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.ticket == other.ticket
    }
}

impl Eq for Waiting {}

impl PartialOrd for Waiting {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for Waiting {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.ticket.cmp(&self.ticket))
    }
}

#[derive(Default)]
struct GateState {
    holding: bool,
    waiters: BinaryHeap<Waiting>,
    next_ticket: u64,
    /// 每次持有者释放时递增。后台等待者据此区分「等到了一次释放」与「被其它
    /// 唤醒但闸门仍被占用」。
    release_generation: u64,
    /// 正在 [`wait_for_write_gate_release`] 里挂起的线程数，仅用于测试确认等待
    /// 已经开始。
    release_waiters: usize,
}

struct Gate {
    state: Mutex<GateState>,
    wake: Condvar,
    /// 已从注册表移除：早先取到 `Arc` 的线程必须重新取闸门，避免同一仓库出现两把。
    retired: AtomicBool,
}

/// 单次获取的结论：抢到闸门，或发现闸门已被回收需要重试。
enum GateOutcome {
    Acquired,
    Retired,
}

impl Gate {
    fn lock_state(&self) -> MutexGuard<'_, GateState> {
        // 取锁期间发生 panic（例如非重入断言）不应让闸门永久不可用。
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn acquire(
        &self,
        priority: WritePriority,
        deadline: Option<Instant>,
        cancel: &CancelToken,
    ) -> Result<GateOutcome, GateError> {
        let mut state = self.lock_state();
        // 闸门可能已被回收：`gate_entry` 返回 `Arc` 之后、进入这里之前，持锁者可能
        // 已经把空闸门移出注册表。此时必须放弃这把闸门（改由调用方重新取），否则
        // `Try` 路径会抢到一把「僵尸闸门」，与随后新建的闸门同时被两个写者持有。
        // 复查与下面的置位同处一段持有闸门锁的临界区，因此与 `Drop` 的回收互斥。
        if self.retired.load(Ordering::SeqCst) {
            return Ok(GateOutcome::Retired);
        }
        // 不排队：`Try` 策略与后台优先级都只看当下是否可用，永不登记等待者。
        let Some(deadline) = deadline.filter(|_| priority != WritePriority::Background) else {
            if state.holding || !state.waiters.is_empty() {
                return Err(GateError::Busy);
            }
            state.holding = true;
            return Ok(GateOutcome::Acquired);
        };

        let ticket = state.next_ticket;
        state.next_ticket = state.next_ticket.wrapping_add(1);
        state.waiters.push(Waiting { priority, ticket });
        loop {
            if self.retired.load(Ordering::SeqCst) {
                state.waiters.retain(|waiter| waiter.ticket != ticket);
                drop(state);
                self.wake.notify_all();
                return Ok(GateOutcome::Retired);
            }
            // 只允许队首进入：队首由（优先级, 登记顺序）决定。
            if !state.holding
                && state
                    .waiters
                    .peek()
                    .is_some_and(|waiter| waiter.ticket == ticket)
            {
                state.waiters.retain(|waiter| waiter.ticket != ticket);
                state.holding = true;
                return Ok(GateOutcome::Acquired);
            }
            if cancel.is_cancelled() {
                state.waiters.retain(|waiter| waiter.ticket != ticket);
                drop(state);
                self.wake.notify_all();
                return Err(GateError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                state.waiters.retain(|waiter| waiter.ticket != ticket);
                drop(state);
                self.wake.notify_all();
                return Err(GateError::TimedOut);
            }
            let timeout = (deadline - now).min(GATE_POLL_INTERVAL);
            let (next, _) = self
                .wake
                .wait_timeout(state, timeout)
                .unwrap_or_else(|poison| poison.into_inner());
            state = next;
        }
    }
}

thread_local! {
    /// 本线程已持有的闸门键，用于检测同线程重入（debug 构建直接断言失败）。
    static HELD_GATES: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

fn is_reentrant(key: &Path) -> bool {
    HELD_GATES.with(|held| held.borrow().iter().any(|entry| entry == key))
}

fn mark_held(key: &Path) {
    HELD_GATES.with(|held| held.borrow_mut().push(key.to_path_buf()));
}

fn unmark_held(key: &Path) {
    HELD_GATES.with(|held| {
        let mut held = held.borrow_mut();
        if let Some(index) = held.iter().position(|entry| entry == key) {
            held.remove(index);
        }
    });
}

static GATES: OnceLock<Mutex<HashMap<PathBuf, Arc<Gate>>>> = OnceLock::new();

/// 等待该仓库的进程内写入闸门被释放，最多等待 `timeout`。返回 `true` 表示
/// 观测到一次释放（或等待开始时闸门已经空闲），`false` 表示超时或被取消。
/// 超时只是防信号丢失，调用方仍应重新检查闸门。
pub(crate) fn wait_for_write_gate_release(
    root: &Path,
    timeout: Duration,
    cancel: &CancelToken,
) -> bool {
    let gate = {
        let registry = gate_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.get(&gate_key(root)).cloned()
    };
    // 闸门已经回收：没有本进程写者占用，按「已释放」处理。
    let Some(gate) = gate else {
        return true;
    };
    let deadline = Instant::now() + timeout;
    let mut state = gate.lock_state();
    let start = state.release_generation;
    if !state.holding {
        return true;
    }
    state.release_waiters += 1;
    let observed = loop {
        if !state.holding || state.release_generation != start {
            break true;
        }
        if cancel.is_cancelled() {
            break false;
        }
        let now = Instant::now();
        if now >= deadline {
            break false;
        }
        let (next, _) = gate
            .wake
            .wait_timeout(state, deadline - now)
            .unwrap_or_else(|poison| poison.into_inner());
        state = next;
    };
    state.release_waiters -= 1;
    observed
}

/// 只在测试里使用：确认某个线程已经真正进入等待，避免「通知早于等待」造成
/// 假阴性。
#[cfg(test)]
pub(crate) fn gate_release_waiter_count(root: &Path) -> usize {
    let gate = {
        let registry = gate_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.get(&gate_key(root)).cloned()
    };
    gate.map_or(0, |gate| gate.lock_state().release_waiters)
}

/// 唤醒所有等待闸门释放的后台任务。应用退出时调用，避免调度线程在兜底超时前
/// 无法被 join。
pub(crate) fn wake_write_gate_waiters() {
    let gates = {
        let registry = gate_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.values().cloned().collect::<Vec<_>>()
    };
    for gate in gates {
        gate.wake.notify_all();
    }
}

fn gate_registry() -> &'static Mutex<HashMap<PathBuf, Arc<Gate>>> {
    GATES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 闸门键：`canonicalize` 让 `F:\repo`、`f:\REPO` 与短路径映射到同一把闸门；
/// 路径尚不存在时退回原路径（此时两侧的判定同样只是「目录不存在」，不进入闸门）。
fn gate_key(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

fn gate_entry(root: &Path) -> (PathBuf, Arc<Gate>) {
    let key = gate_key(root);
    let mut registry = gate_registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(gate) = registry.get(&key) {
        return (key, gate.clone());
    }
    let gate = Arc::new(Gate {
        state: Mutex::new(GateState::default()),
        wake: Condvar::new(),
        retired: AtomicBool::new(false),
    });
    registry.insert(key.clone(), gate.clone());
    (key, gate)
}

/// 取得进程内写入闸门。返回的守卫在 `Drop` 时释放并唤醒下一个等待者。
///
/// 排队顺序：Shutdown > Foreground > Interactive；同优先级先到先得。
/// `Background` 与 [`WaitPolicy::Try`] 不排队，占用时立即返回 [`GateError::Busy`]。
pub(crate) fn acquire_write_gate(
    root: &Path,
    priority: WritePriority,
    wait: WaitPolicy,
    cancel: &CancelToken,
) -> Result<WriteGateGuard, GateError> {
    let deadline = match wait {
        WaitPolicy::Try => None,
        WaitPolicy::Until(at) => Some(at),
    };
    loop {
        let (key, gate) = gate_entry(root);
        if is_reentrant(&key) {
            // 非重入是调用方缺陷：生产环境直接失败（不能等自己），debug 构建断言。
            log::error!(
                "[gate] re-entrant repository write gate acquisition on the same thread: path={}",
                key.display()
            );
            debug_assert!(false, "repository write gate is not re-entrant");
            return Err(GateError::Reentrant);
        }
        match gate.acquire(priority, deadline, cancel) {
            Ok(GateOutcome::Acquired) => {
                mark_held(&key);
                return Ok(WriteGateGuard { key, gate });
            }
            // 闸门已被回收（取到 `Arc` 之后才被回收，或等待期间被回收）：重新从
            // 注册表取新闸门。被回收的闸门已移出注册表，因此这里最多重试一次。
            Ok(GateOutcome::Retired) => continue,
            Err(error) => return Err(error),
        }
    }
}

/// 进程内写入闸门的持有证明。`Drop` 时释放并把等待者从空闸门里清理掉。
#[must_use = "持锁证明必须持有到写操作结束；立即丢弃等于没有互斥"]
pub(crate) struct WriteGateGuard {
    key: PathBuf,
    gate: Arc<Gate>,
}

impl std::fmt::Debug for WriteGateGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriteGateGuard")
            .field("root", &self.key)
            .finish()
    }
}

/// 当前登记在某仓库闸门上的等待者数量。只用于测试断言「等待者已出队」。
#[cfg(test)]
pub(crate) fn gate_waiter_count(root: &Path) -> usize {
    let key = gate_key(root);
    let registry = gate_registry()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    registry
        .get(&key)
        .map_or(0, |gate| gate.lock_state().waiters.len())
}

impl Drop for WriteGateGuard {
    fn drop(&mut self) {
        unmark_held(&self.key);
        {
            let mut state = self.gate.lock_state();
            state.holding = false;
            state.release_generation = state.release_generation.wrapping_add(1);
        }
        self.gate.wake.notify_all();
        // 空闲闸门从注册表移除：切仓库后不泄漏，内存也不会随仓库数增长。
        // 锁序固定为「注册表 → 闸门」，与获取路径一致。判定空闲、标记 retired 与
        // 移出注册表在同一段持有闸门锁的临界区里完成，与获取路径的「查 retired →
        // 置 holding」互斥，因此不会出现「一边回收、另一边却抢到已被移出的闸门」。
        let mut registry = gate_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let state = self.gate.lock_state();
        if state.holding || !state.waiters.is_empty() {
            return;
        }
        if registry
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.gate))
        {
            self.gate.retired.store(true, Ordering::SeqCst);
            registry.remove(&self.key);
        }
    }
}

/// 持有期间独占仓库写入的 owner lock；随连接一起在 `Drop` 时释放。
#[must_use = "持锁证明必须持有到写操作结束；立即丢弃等于没有互斥"]
pub(crate) struct RepositoryOwnerLock {
    connection: Option<Connection>,
    owner_path: PathBuf,
    heartbeat_stop: Option<mpsc::Sender<()>>,
    heartbeat_thread: Option<thread::JoinHandle<()>>,
    /// 进程内排队闸门。`acquire`（零等待、不排队）保持 `None`。
    /// 声明在最后：`Drop` 里先 `ROLLBACK` 释放 SQLite 锁，再放闸门。
    gate: Option<WriteGateGuard>,
}

impl RepositoryOwnerLock {
    /// 对一次仓库写操作取得独占锁。调用方必须保证 `root` 已存在。
    ///
    /// 零等待、不排队：`probe` 之外的调用方应优先用 [`Self::acquire_queued`]，
    /// 否则同进程的两条写路径会互相以 `repository_busy` 拒绝。headless 是单进程
    /// 单命令，仍然用它。
    #[cfg_attr(not(feature = "headless"), allow(dead_code))] // headless 与测试使用。
    pub(crate) fn acquire(
        root: &Path,
        role: &str,
        operation: &str,
    ) -> Result<Self, OwnerLockError> {
        Self::acquire_locked(root, role, operation, None)
    }

    /// 先在本进程的写入闸门里排队，再取跨进程 owner lock。
    ///
    /// 跨进程语义不变：SQLite 侧仍是零等待的 `BEGIN IMMEDIATE`，另一个进程持锁时
    /// 立即返回 [`OwnerLockError::Busy`]（此时刚取得的闸门随之释放）。
    pub(crate) fn acquire_queued(
        root: &Path,
        role: &str,
        operation: &str,
        priority: WritePriority,
        wait: WaitPolicy,
        cancel: &CancelToken,
    ) -> Result<Self, OwnerLockError> {
        if !root.is_dir() {
            return Err(OwnerLockError::Io("作品仓库目录不存在".to_owned()));
        }
        let gate =
            acquire_write_gate(root, priority, wait, cancel).map_err(owner_lock_gate_error)?;
        Self::acquire_locked(root, role, operation, Some(gate))
    }

    fn acquire_locked(
        root: &Path,
        role: &str,
        operation: &str,
        gate: Option<WriteGateGuard>,
    ) -> Result<Self, OwnerLockError> {
        if !root.is_dir() {
            return Err(OwnerLockError::Io("作品仓库目录不存在".to_owned()));
        }
        let connection = open_lock_database(root).map_err(OwnerLockError::Io)?;
        match connection.execute_batch("BEGIN IMMEDIATE;") {
            Ok(()) => {}
            Err(rusqlite::Error::SqliteFailure(inner, _))
                if inner.code == rusqlite::ErrorCode::DatabaseBusy =>
            {
                let owner = read_owner(root);
                match &owner {
                    Some(owner) => log::warn!(
                        "[owner] repository write lock busy: {} same_process={}",
                        owner.describe(),
                        owner.pid == process::id()
                    ),
                    None => {
                        log::warn!("[owner] repository write lock busy: owner metadata unavailable")
                    }
                }
                // gate 随局部变量一起在这里释放：跨进程拒绝不占用进程内排队位。
                return Err(OwnerLockError::Busy(owner));
            }
            Err(error) => {
                return Err(OwnerLockError::Io(format!("无法取得仓库写锁：{error}")));
            }
        }

        // 取得内核锁即可证明旧 owner 已失效（含未清理的强杀残留）。直接覆盖其
        // 元数据并留下一条可检索的记录，不做 PID 判定。
        if let Some(stale) = read_owner(root) {
            log::warn!("[owner] replacing stale owner: {}", stale.describe());
        }
        let record = RepositoryOwner {
            pid: process::id(),
            process_started_ms: process_started_ms(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            role: role.to_owned(),
            operation: operation.to_owned(),
            run_id: crate::app::diagnostics::run_id().to_owned(),
            heartbeat_ms: now_ms(),
        };
        let owner_path = root.join(OWNER_FILE_NAME);
        if let Err(error) = write_owner_file(&owner_path, &record) {
            // 元数据失败不影响锁本身；只留下告警。
            log::warn!("[owner] failed to write owner metadata: {error}");
        }
        log::debug!(
            "[owner] acquired: pid={}, role={}, operation={}",
            record.pid,
            record.role,
            record.operation
        );

        let record = Arc::new(Mutex::new(record));
        let (stop_tx, stop_rx) = mpsc::channel();
        let heartbeat_path = owner_path.clone();
        let heartbeat_thread = thread::spawn(move || loop {
            match stop_rx.recv_timeout(HEARTBEAT_INTERVAL) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let next = match record.lock() {
                        Ok(mut record) => {
                            record.heartbeat_ms = now_ms();
                            record.clone()
                        }
                        Err(_) => break,
                    };
                    if let Err(error) = write_owner_file(&heartbeat_path, &next) {
                        log::warn!("[owner] failed to refresh owner heartbeat: {error}");
                    }
                }
            }
        });

        Ok(Self {
            connection: Some(connection),
            owner_path,
            heartbeat_stop: Some(stop_tx),
            heartbeat_thread: Some(heartbeat_thread),
            gate,
        })
    }
}

impl std::fmt::Debug for RepositoryOwnerLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepositoryOwnerLock")
            .field("owner_path", &self.owner_path)
            .finish()
    }
}

impl Drop for RepositoryOwnerLock {
    fn drop(&mut self) {
        if let Some(stop) = self.heartbeat_stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.heartbeat_thread.take() {
            let _ = handle.join();
        }
        // 先移除元数据再释放锁：下一个 owner 取得锁时不会误读本进程的残留。
        let _ = fs::remove_file(&self.owner_path);
        if let Some(connection) = self.connection.take() {
            let _ = connection.execute_batch("ROLLBACK;");
        }
        // 释放顺序：先放 SQLite 锁，再放进程内闸门。反过来会让下一个本进程写者
        // 先拿到闸门、再被自己进程的 SQLite 锁拒绝。
        drop(self.gate.take());
    }
}

fn owner_lock_gate_error(error: GateError) -> OwnerLockError {
    match error {
        GateError::Busy => OwnerLockError::InProcessBusy,
        GateError::TimedOut => OwnerLockError::TimedOut,
        GateError::Cancelled => OwnerLockError::Cancelled,
        GateError::Reentrant => {
            OwnerLockError::Io("仓库写入闸门不可重入（内部错误，请报告该缺陷）".to_owned())
        }
    }
}

/// 探测写锁是否被占用：`BEGIN IMMEDIATE` 成功即说明此刻没有写者，随后立即回滚，
/// 不把只读命令变成写者之间的互斥来源。
pub(crate) fn probe(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let path = root.join(LOCK_FILE_NAME);
    // 哨兵库还不存在，说明没有任何进程用过写锁。
    if !path.is_file() {
        return Ok(());
    }
    let connection = open_lock_database(root)?;
    match connection.execute_batch("BEGIN IMMEDIATE;") {
        Ok(()) => connection
            .execute_batch("ROLLBACK;")
            .map_err(|error| format!("无法释放仓库哨兵探测：{error}")),
        Err(rusqlite::Error::SqliteFailure(inner, _))
            if inner.code == rusqlite::ErrorCode::DatabaseBusy =>
        {
            if let Some(owner) = read_owner(root) {
                log::warn!(
                    "[owner] content read refused while a writer holds the lock: {}",
                    owner.describe()
                );
            }
            Err(CONTENT_READ_BUSY_MESSAGE.to_owned())
        }
        Err(other) => Err(format!("无法检查仓库写锁：{other}")),
    }
}

/// 仓库根目录下的锁库、锁日志与 owner 元数据属于进程级运行时文件，不进入灾备
/// 副本，也不参与未引用文件判定。
pub(crate) fn is_runtime_artifact(relative: &Path) -> bool {
    relative
        .parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
        && relative.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name == OWNER_FILE_NAME || name.starts_with(".lilith-write-lock")
        })
}

fn owner_path(root: &Path) -> PathBuf {
    root.join(OWNER_FILE_NAME)
}

fn read_owner(root: &Path) -> Option<RepositoryOwner> {
    let path = owner_path(root);
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_owner_file(path: &Path, owner: &RepositoryOwner) -> Result<(), String> {
    // 直接覆盖写入而不是临时文件重命名：心跳发生在仓库写操作期间，临时文件会
    // 出现在根目录并被灾备扫描看到。元数据仅是诊断信息，读到半截时可安全忽略。
    let bytes = serde_json::to_vec_pretty(owner)
        .map_err(|error| format!("无法序列化 owner 元数据：{error}"))?;
    fs::write(path, bytes).map_err(|error| format!("无法写入 owner 元数据：{error}"))
}

/// 打开哨兵锁数据库并配置为零等待：这里要的是「立刻判定是否被占用」，而不是排队。
/// 真实写入仍由主库自己的 `busy_timeout` 承担。
fn open_lock_database(root: &Path) -> Result<Connection, String> {
    let path = root.join(LOCK_FILE_NAME);
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|error| format!("无法打开仓库哨兵锁：{error}"))?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(|error| format!("无法配置仓库哨兵锁：{error}"))?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::mpsc::{self, Receiver, Sender},
        time::Instant,
    };

    use super::*;

    /// 测试用的排队上限：足够覆盖正常路径，又能让「等不到」的用例快速失败。
    const TEST_WAIT: Duration = Duration::from_secs(5);

    /// 轮询直到条件成立（等待者登记是另一线程的行为，无法从外部确认时刻）。
    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + TEST_WAIT;
        while !condition() {
            assert!(Instant::now() < deadline, "条件在超时前未成立");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// 在一个线程里排队取闸门，抢到后报告标签并等释放信号。
    fn spawn_gate_waiter(
        root: &Path,
        priority: WritePriority,
        label: &'static str,
        entered: Sender<&'static str>,
        release: Receiver<()>,
    ) -> thread::JoinHandle<()> {
        let root = root.to_path_buf();
        thread::spawn(move || {
            let guard = acquire_write_gate(
                &root,
                priority,
                WaitPolicy::Until(Instant::now() + TEST_WAIT),
                &CancelToken::standalone(),
            )
            .expect("排队应在上限内取得闸门");
            entered.send(label).unwrap();
            let _ = release.recv_timeout(TEST_WAIT);
            drop(guard);
        })
    }

    #[test]
    fn a_second_writer_is_refused_and_release_restores_acquisition() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();

        let writer = RepositoryOwnerLock::acquire(root, "gui", "library:write").unwrap();
        let error = match RepositoryOwnerLock::acquire(root, "headless", "commit") {
            Ok(_) => panic!("第二个写者必须被拒绝"),
            Err(error) => error,
        };
        match error {
            OwnerLockError::Busy(owner) => {
                let owner = owner.expect("侧车元数据应可读");
                assert_eq!(owner.operation, "library:write");
                assert_eq!(owner.app_version, env!("CARGO_PKG_VERSION"));
            }
            other => panic!("应得到占用错误而不是 IO 错误：{other:?}"),
        }

        drop(writer);
        let _ =
            RepositoryOwnerLock::acquire(root, "headless", "commit").expect("释放后应能重新取得锁");
    }

    #[test]
    fn queued_waiters_enter_in_registration_order() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let cancel = CancelToken::standalone();
        let holder =
            acquire_write_gate(root, WritePriority::Interactive, WaitPolicy::Try, &cancel).unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (release_second_tx, release_second_rx) = mpsc::channel();
        let first = spawn_gate_waiter(
            root,
            WritePriority::Interactive,
            "first",
            entered_tx.clone(),
            release_first_rx,
        );
        wait_until(|| gate_waiter_count(root) == 1);
        let second = spawn_gate_waiter(
            root,
            WritePriority::Interactive,
            "second",
            entered_tx,
            release_second_rx,
        );
        wait_until(|| gate_waiter_count(root) == 2);

        drop(holder);
        assert_eq!(
            entered_rx.recv_timeout(TEST_WAIT).unwrap(),
            "first",
            "同优先级必须按登记顺序进入"
        );
        assert!(
            entered_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "第二个等待者不得在第一个释放前进入"
        );

        release_first_tx.send(()).unwrap();
        assert_eq!(entered_rx.recv_timeout(TEST_WAIT).unwrap(), "second");
        release_second_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
        assert_eq!(gate_waiter_count(root), 0);
    }

    #[test]
    fn a_more_urgent_waiter_overtakes_the_queue() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let cancel = CancelToken::standalone();
        let holder =
            acquire_write_gate(root, WritePriority::Interactive, WaitPolicy::Try, &cancel).unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_interactive_tx, release_interactive_rx) = mpsc::channel();
        let (release_foreground_tx, release_foreground_rx) = mpsc::channel();
        let interactive = spawn_gate_waiter(
            root,
            WritePriority::Interactive,
            "interactive",
            entered_tx.clone(),
            release_interactive_rx,
        );
        wait_until(|| gate_waiter_count(root) == 1);
        let foreground = spawn_gate_waiter(
            root,
            WritePriority::Foreground,
            "foreground",
            entered_tx,
            release_foreground_rx,
        );
        wait_until(|| gate_waiter_count(root) == 2);

        drop(holder);
        assert_eq!(
            entered_rx.recv_timeout(TEST_WAIT).unwrap(),
            "foreground",
            "后到但更紧急的写者必须插队"
        );
        release_foreground_tx.send(()).unwrap();
        assert_eq!(entered_rx.recv_timeout(TEST_WAIT).unwrap(), "interactive");
        release_interactive_tx.send(()).unwrap();
        foreground.join().unwrap();
        interactive.join().unwrap();
    }

    #[test]
    fn waiting_past_the_deadline_times_out_and_leaves_the_queue() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let holder = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .unwrap();

        // 等待者在另一个线程：同线程重入会被非重入断言直接拦下（见对应用例）。
        let waiter_root = root.to_path_buf();
        let (error, elapsed) = thread::spawn(move || {
            let started = Instant::now();
            let error = acquire_write_gate(
                &waiter_root,
                WritePriority::Interactive,
                WaitPolicy::Until(Instant::now() + Duration::from_millis(50)),
                &CancelToken::standalone(),
            )
            .unwrap_err();
            (error, started.elapsed())
        })
        .join()
        .unwrap();

        assert_eq!(error, GateError::TimedOut);
        assert!(elapsed < TEST_WAIT, "超时必须按截止时刻返回");
        assert_eq!(gate_waiter_count(root), 0, "超时的等待者必须出队");
        assert!(!owner_path(root).exists(), "超时方不得写入 owner 元数据");

        drop(holder);
        let _ = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .expect("释放后闸门必须可用");
    }

    #[test]
    fn a_cancelled_waiter_leaves_the_queue_without_a_ghost_wakeup() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let holder = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .unwrap();

        let cancel = CancelToken::standalone();
        let waiter_cancel = cancel.clone();
        let waiter_root = root.to_path_buf();
        let waiter = thread::spawn(move || {
            acquire_write_gate(
                &waiter_root,
                WritePriority::Interactive,
                WaitPolicy::Until(Instant::now() + TEST_WAIT),
                &waiter_cancel,
            )
        });
        wait_until(|| gate_waiter_count(root) == 1);
        cancel.cancel();

        assert_eq!(waiter.join().unwrap().unwrap_err(), GateError::Cancelled);
        assert_eq!(gate_waiter_count(root), 0, "取消的等待者必须出队");
        drop(holder);

        // 后续写者不会被「幽灵唤醒」抢走或误判占用。
        let next = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Until(Instant::now() + TEST_WAIT),
            &CancelToken::standalone(),
        )
        .expect("释放后闸门必须可用");
        drop(next);
    }

    #[test]
    fn a_cross_process_writer_still_refuses_immediately() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // 线程 A 用零等待的 `acquire` 持锁（不经过闸门），模拟另一个进程的写者。
        let writer = RepositoryOwnerLock::acquire(root, "headless", "compact").unwrap();

        let started = Instant::now();
        let error = RepositoryOwnerLock::acquire_queued(
            root,
            "gui",
            "library:write",
            WritePriority::Interactive,
            WaitPolicy::Until(Instant::now() + Duration::from_secs(2)),
            &CancelToken::standalone(),
        )
        .unwrap_err();

        match error {
            OwnerLockError::Busy(owner) => {
                assert_eq!(owner.expect("侧车元数据应可读").operation, "compact")
            }
            other => panic!("跨进程占用必须立即拒绝：{other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "跨进程冲突不得排队等待"
        );
        // 跨进程拒绝必须把刚取得的闸门还回去，否则本进程会被自己的失败卡住。
        assert_eq!(gate_waiter_count(root), 0);

        drop(writer);
        let _ = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .expect("跨进程拒绝后闸门必须已经释放");
    }

    #[test]
    fn dropping_a_queued_lock_releases_the_gate_and_the_owner_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let lock = RepositoryOwnerLock::acquire_queued(
            root,
            "gui",
            "pinBoard:finalize",
            WritePriority::Shutdown,
            WaitPolicy::Until(Instant::now() + TEST_WAIT),
            &CancelToken::standalone(),
        )
        .unwrap();
        assert!(owner_path(root).exists());

        drop(lock);
        assert!(!owner_path(root).exists());
        let _ = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .expect("锁对象销毁后闸门必须可用");
    }

    #[cfg(windows)]
    #[test]
    fn gates_are_keyed_by_the_canonical_path() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let holder = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .unwrap();

        // 大小写变体必须映射到同一把闸门，否则同一仓库会出现两把互不相认的闸门。
        // 从另一个线程尝试，避免踩到同线程的非重入断言。
        let variant = PathBuf::from(root.to_string_lossy().to_uppercase());
        let probe = variant.clone();
        let error = thread::spawn(move || {
            acquire_write_gate(
                &probe,
                WritePriority::Interactive,
                WaitPolicy::Try,
                &CancelToken::standalone(),
            )
            .unwrap_err()
        })
        .join()
        .unwrap();
        assert_eq!(error, GateError::Busy);

        drop(holder);
        let _ = acquire_write_gate(
            &variant,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .expect("释放后变体路径同样可用");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "not re-entrant")]
    fn the_gate_is_not_reentrant() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let _first = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .unwrap();
        // 同线程再取一次会等自己：debug 构建必须断言失败，而不是死锁。
        let _second = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Until(Instant::now() + TEST_WAIT),
            &CancelToken::standalone(),
        )
        .unwrap();
    }

    #[test]
    fn stale_owner_metadata_is_overwritten_once_the_kernel_lock_is_free() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let stale = RepositoryOwner {
            pid: 999_999,
            process_started_ms: 1,
            app_version: "0.0.0-stale".into(),
            role: "headless".into(),
            operation: "crashed-commit".into(),
            run_id: "stale".into(),
            heartbeat_ms: 1,
        };
        write_owner_file(&owner_path(root), &stale).unwrap();

        // 锁未被占用：stale 只表示上一次进程崩溃后残留的元数据，取得锁即覆盖。
        let guard = RepositoryOwnerLock::acquire(root, "gui", "settings:switch").unwrap();
        let current = read_owner(root).expect("取得锁后应写入新的 owner 元数据");
        assert_eq!(current.role, "gui");
        assert_eq!(current.operation, "settings:switch");
        assert_ne!(current.pid, stale.pid);

        drop(guard);
        assert!(!owner_path(root).exists());
    }

    #[test]
    fn content_read_probe_is_refused_while_a_writer_holds_the_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(probe(root).is_ok());

        let writer = RepositoryOwnerLock::acquire(root, "headless", "compact").unwrap();
        let error = probe(root).unwrap_err();
        assert!(error.contains("仓库正在被写入"), "{error}");

        drop(writer);
        assert!(probe(root).is_ok());
    }

    #[test]
    fn waiting_for_a_gate_release_is_woken_by_the_release() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let holder = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .unwrap();

        let waiter_root = root.to_path_buf();
        let waiter = thread::spawn(move || {
            wait_for_write_gate_release(&waiter_root, TEST_WAIT, &CancelToken::standalone())
        });
        wait_until(|| gate_release_waiter_count(root) == 1);

        let started = Instant::now();
        drop(holder);
        assert!(waiter.join().unwrap(), "释放闸门必须唤醒等待者");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn waiting_for_a_gate_release_times_out_without_a_release() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let holder = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .unwrap();

        let started = Instant::now();
        assert!(!wait_for_write_gate_release(
            root,
            Duration::from_millis(30),
            &CancelToken::standalone(),
        ));
        assert!(started.elapsed() >= Duration::from_millis(25));
        drop(holder);
    }

    #[test]
    fn waiting_for_a_gate_release_stops_when_the_app_shuts_down() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let holder = acquire_write_gate(
            root,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        )
        .unwrap();
        let cancel = CancelToken::standalone();
        let waiter_cancel = cancel.clone();
        let waiter_root = root.to_path_buf();
        let waiter = thread::spawn(move || {
            wait_for_write_gate_release(&waiter_root, TEST_WAIT, &waiter_cancel)
        });
        wait_until(|| gate_release_waiter_count(root) == 1);

        cancel.cancel();
        wake_write_gate_waiters();
        assert!(!waiter.join().unwrap(), "退出流程不得被 5 秒兜底拖住");
        drop(holder);
    }

    #[test]
    fn runtime_artifacts_are_kept_out_of_repository_content() {
        assert!(is_runtime_artifact(Path::new(LOCK_FILE_NAME)));
        assert!(is_runtime_artifact(Path::new(&format!(
            "{LOCK_FILE_NAME}-journal"
        ))));
        assert!(is_runtime_artifact(Path::new(OWNER_FILE_NAME)));
        assert!(!is_runtime_artifact(Path::new("lilith-artworks.sqlite3")));
        assert!(!is_runtime_artifact(Path::new(
            "artworks/artwork-1/.lilith-write-owner.json"
        )));
    }
}
