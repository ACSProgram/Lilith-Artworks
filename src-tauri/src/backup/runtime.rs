use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

use serde::Serialize;
use tauri::AppHandle;

use super::BackupRuntimeStatus;

/// 当前占用共享运行锁的任务类型。
///
/// 前端据此区分"用户触发的关键操作"与"后台低优先级任务"，取消路由也据此判断
/// 前台命令请求让位时能否取消当前任务。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum BackupTaskKind {
    /// 调度器发起的到期自动备份。
    AutomaticBackup,
    /// 调度器发起的空闲链路校验。
    IdleVerify,
    /// 用户触发的前台命令。
    UserOperation,
}

impl BackupTaskKind {
    /// 后台任务可以让位于前台命令；前台任务只能由用户主动取消。
    fn is_background(self) -> bool {
        matches!(self, Self::AutomaticBackup | Self::IdleVerify)
    }
}

#[derive(Debug)]
pub(crate) enum ExclusiveRunError<E> {
    ShuttingDown,
    State(String),
    Operation(E),
}

impl<E: std::fmt::Display> std::fmt::Display for ExclusiveRunError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShuttingDown => formatter.write_str("应用正在退出，操作已取消"),
            Self::State(error) => formatter.write_str(error),
            Self::Operation(error) => error.fmt(formatter),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct BackupState {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    operation_lock: Mutex<()>,
    runtime: Mutex<BackupRuntimeStatus>,
    cancel_requested: AtomicBool,
    shutting_down: AtomicBool,
    /// 手动提交优先：记录有待处理手动提交请求的分支 ID，调度器据此延后
    /// 这些分支的自动备份，避免自动任务抢在手动提交之前占据共享运行锁。
    manual_pending: Mutex<HashSet<String>>,
    /// 正在等待共享运行锁的前台长命令数量。调度器据此在候选选择与取得运行锁后
    /// 让位，避免后台任务抢先取锁并清掉前台刚设置的取消标志。
    foreground_waiting: AtomicUsize,
    scheduler_signal: Mutex<SchedulerSignal>,
    scheduler_wake: Condvar,
    scheduler_handle: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct SchedulerSignal {
    stop: bool,
    generation: u64,
}

impl BackupState {
    pub(crate) fn set_automatic_scheduling(&self, enabled: bool) {
        if let Ok(mut runtime) = self.inner.runtime.lock() {
            runtime.automatic_scheduling = enabled;
        }
    }

    pub(crate) fn begin_manual_request(&self, branch_id: &str) {
        if let Ok(mut pending) = self.inner.manual_pending.lock() {
            pending.insert(branch_id.to_owned());
        }
    }

    pub(crate) fn end_manual_request(&self, branch_id: &str) {
        if let Ok(mut pending) = self.inner.manual_pending.lock() {
            pending.remove(branch_id);
        }
    }

    pub(crate) fn manual_pending(&self, branch_id: &str) -> bool {
        self.inner
            .manual_pending
            .lock()
            .map(|pending| pending.contains(branch_id))
            .unwrap_or(false)
    }

    /// 是否存在任意分支待处理的手动提交。空闲校验据此整体让位：用户正在主动
    /// 提交时不与之争抢运行锁。
    pub(crate) fn any_manual_pending(&self) -> bool {
        self.inner
            .manual_pending
            .lock()
            .map(|pending| !pending.is_empty())
            .unwrap_or(false)
    }

    /// 登记一个正在等待共享运行锁的前台命令。
    ///
    /// 计数器由 `ForegroundWait::release` 在取得运行锁后递减，因此它只反映"仍在
    /// 排队"的前台命令，不包含已经取得锁正在执行的命令。调度器读它来决定让位。
    pub(crate) fn begin_foreground_wait(&self) -> ForegroundWait {
        self.inner.foreground_waiting.fetch_add(1, Ordering::SeqCst);
        ForegroundWait {
            state: self.clone(),
            active: true,
        }
    }

    pub(crate) fn foreground_waiting(&self) -> usize {
        self.inner.foreground_waiting.load(Ordering::SeqCst)
    }

    /// 请求正在运行的后台任务让位；前台任务不受影响，仍只能由 `request_cancel`
    /// 取消。返回是否实际发出了取消请求。
    pub(crate) fn cancel_background(&self) -> Result<bool, String> {
        let runtime = self.inner.runtime.lock().map_err(|_| "备份状态已损坏")?;
        let background =
            runtime.busy && runtime.task_kind.is_some_and(BackupTaskKind::is_background);
        if background {
            self.inner.cancel_requested.store(true, Ordering::SeqCst);
        }
        Ok(background)
    }

    /// 前台长命令入口：先登记等待并请求后台任务让位，再取得共享运行锁。
    ///
    /// 与 `run_exclusive` 的差别只有这两步，都不阻塞、不排队：登记让调度器在取得
    /// 运行锁后复查并让位，取消请求让正在运行的后台任务尽快退出。
    pub(crate) fn run_foreground<T>(
        &self,
        branch_id: Option<&str>,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let mut waiting = self.begin_foreground_wait();
        let _ = self.cancel_background();
        self.run_exclusive_typed_inner(
            branch_id,
            BackupTaskKind::UserOperation,
            Some(&mut waiting),
            operation,
        )
        .map_err(|error| error.to_string())
    }

    pub(crate) fn run_exclusive<T>(
        &self,
        branch_id: Option<&str>,
        kind: BackupTaskKind,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.run_exclusive_typed(branch_id, kind, operation)
            .map_err(|error| error.to_string())
    }

    /// Runs a long operation under the exclusive lock and records its duration
    /// and outcome. Every repository-heavy command goes through this so a slow
    /// or failing materialization is always attributable in the log.
    pub(crate) fn run_logged<T>(
        &self,
        name: &str,
        detail: &str,
        branch_id: Option<&str>,
        kind: BackupTaskKind,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.logged(name, detail, || {
            self.run_exclusive(branch_id, kind, operation)
        })
    }

    /// 前台长命令版本的 `run_logged`：登记前台等待并请求后台任务让位。
    pub(crate) fn run_logged_foreground<T>(
        &self,
        name: &str,
        detail: &str,
        branch_id: Option<&str>,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.logged(name, detail, || self.run_foreground(branch_id, operation))
    }

    fn logged<T>(
        &self,
        name: &str,
        detail: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let started = std::time::Instant::now();
        log::info!("{name} started: {detail}");
        let result = operation();
        match &result {
            Ok(_) => log::info!(
                "{name} finished: {detail}, elapsed_ms={}",
                started.elapsed().as_millis()
            ),
            Err(error) => log::warn!(
                "{name} failed: {detail}, elapsed_ms={}, error={error}",
                started.elapsed().as_millis()
            ),
        }
        result
    }

    pub(crate) fn run_exclusive_typed<T, E>(
        &self,
        branch_id: Option<&str>,
        kind: BackupTaskKind,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, ExclusiveRunError<E>> {
        self.run_exclusive_typed_inner(branch_id, kind, None, operation)
    }

    fn run_exclusive_typed_inner<T, E>(
        &self,
        branch_id: Option<&str>,
        kind: BackupTaskKind,
        foreground_wait: Option<&mut ForegroundWait>,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, ExclusiveRunError<E>> {
        let _guard = self
            .inner
            .operation_lock
            .lock()
            .map_err(|_| ExclusiveRunError::State("备份操作锁已损坏".into()))?;
        if self.inner.shutting_down.load(Ordering::SeqCst) {
            return Err(ExclusiveRunError::ShuttingDown);
        }
        // 已经取得运行锁，前台命令不再属于"等待中"，让调度器恢复正常调度。
        if let Some(waiting) = foreground_wait {
            waiting.release();
        }
        self.inner.cancel_requested.store(false, Ordering::SeqCst);
        {
            let mut runtime = self
                .inner
                .runtime
                .lock()
                .map_err(|_| ExclusiveRunError::State("备份状态已损坏".into()))?;
            runtime.busy = true;
            runtime.active_branch_id = branch_id.map(str::to_owned);
            runtime.task_kind = Some(kind);
        }
        let result = operation().map_err(ExclusiveRunError::Operation);
        if let Ok(mut runtime) = self.inner.runtime.lock() {
            let scheduling = runtime.automatic_scheduling;
            let completion_revision = runtime.completion_revision.wrapping_add(1);
            *runtime = BackupRuntimeStatus {
                automatic_scheduling: scheduling,
                completion_revision,
                ..Default::default()
            };
        }
        if !self.inner.shutting_down.load(Ordering::SeqCst) {
            self.inner.cancel_requested.store(false, Ordering::SeqCst);
        }
        result
    }

    pub(crate) fn status(&self) -> Result<BackupRuntimeStatus, String> {
        self.inner
            .runtime
            .lock()
            .map(|value| value.clone())
            .map_err(|_| "备份状态已损坏".into())
    }

    pub(crate) fn report_progress(&self, operation: &str, label: &str, current: u64, total: u64) {
        if let Ok(mut runtime) = self.inner.runtime.lock() {
            runtime.operation = Some(operation.into());
            runtime.progress_label = Some(label.into());
            runtime.progress_current = current.min(total);
            runtime.progress_total = total;
        }
    }

    pub(crate) fn request_cancel(&self) -> Result<bool, String> {
        let busy = self
            .inner
            .runtime
            .lock()
            .map_err(|_| "备份状态已损坏")?
            .busy;
        if busy {
            self.inner.cancel_requested.store(true, Ordering::SeqCst);
        }
        Ok(busy)
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.inner.cancel_requested.load(Ordering::SeqCst)
    }

    pub(crate) fn start_scheduler(&self, app: AppHandle) -> Result<(), String> {
        let mut handle = self
            .inner
            .scheduler_handle
            .lock()
            .map_err(|_| "备份调度线程状态已损坏")?;
        if handle.is_some() {
            return Ok(());
        }
        self.inner.shutting_down.store(false, Ordering::SeqCst);
        self.inner.cancel_requested.store(false, Ordering::SeqCst);
        if let Ok(mut signal) = self.inner.scheduler_signal.lock() {
            signal.stop = false;
            signal.generation = signal.generation.wrapping_add(1);
        }
        self.set_automatic_scheduling(true);
        let state = self.clone();
        *handle = Some(std::thread::spawn(move || {
            super::scheduler::run(state, app)
        }));
        Ok(())
    }

    pub(crate) fn wake_scheduler(&self) {
        if let Ok(mut signal) = self.inner.scheduler_signal.lock() {
            signal.generation = signal.generation.wrapping_add(1);
            self.inner.scheduler_wake.notify_all();
        }
    }

    pub(crate) fn wait_scheduler(&self, timeout: Duration) -> bool {
        let Ok(signal) = self.inner.scheduler_signal.lock() else {
            return false;
        };
        if signal.stop {
            return false;
        }
        let generation = signal.generation;
        self.inner
            .scheduler_wake
            .wait_timeout_while(signal, timeout, |value| {
                !value.stop && value.generation == generation
            })
            .map(|(value, _)| !value.stop)
            .unwrap_or(false)
    }

    pub(crate) fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        self.inner.cancel_requested.store(true, Ordering::SeqCst);
        if let Ok(mut signal) = self.inner.scheduler_signal.lock() {
            signal.stop = true;
            signal.generation = signal.generation.wrapping_add(1);
            self.inner.scheduler_wake.notify_all();
        }
        if let Some(handle) = self
            .inner
            .scheduler_handle
            .lock()
            .ok()
            .and_then(|mut value| value.take())
        {
            let _ = handle.join();
        }
        let _operation_guard = self.inner.operation_lock.lock().ok();
        if let Ok(mut runtime) = self.inner.runtime.lock() {
            *runtime = BackupRuntimeStatus::default();
        }
    }
}

/// 前台等待登记的生命周期守卫。
///
/// 前台命令在尝试取得共享运行锁之前创建它，并在取得锁后释放；`Drop` 保证取消、
/// 失败、退出和提前返回等所有路径都会把计数归零。
pub(crate) struct ForegroundWait {
    state: BackupState,
    active: bool,
}

impl ForegroundWait {
    fn release(&mut self) {
        if self.active {
            self.active = false;
            self.state
                .inner
                .foreground_waiting
                .fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for ForegroundWait {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        },
        thread,
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn completion_revision_advances_for_short_operations() {
        let state = BackupState::default();
        assert_eq!(state.status().unwrap().completion_revision, 0);

        state
            .run_exclusive(None, BackupTaskKind::UserOperation, || Ok::<_, String>(()))
            .unwrap();
        let first = state.status().unwrap();
        assert!(!first.busy);
        assert_eq!(first.completion_revision, 1);

        state
            .run_exclusive(None, BackupTaskKind::UserOperation, || {
                Err::<(), _>("failed".to_string())
            })
            .unwrap_err();
        assert_eq!(state.status().unwrap().completion_revision, 2);
    }

    /// 在指定任务类型下运行一段操作，期间用 `cancel_background` 请求让位，返回
    /// `(是否发出取消请求, 运行中的任务是否观察到取消标志)`。
    fn probe_background_cancel(kind: BackupTaskKind) -> (bool, bool) {
        let state = BackupState::default();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (observed_tx, observed_rx) = mpsc::channel();
        let running = state.clone();
        let handle = thread::spawn(move || {
            running.run_exclusive(None, kind, || {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                observed_tx.send(running.cancelled()).unwrap();
                Ok::<_, String>(())
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(state.status().unwrap().task_kind, Some(kind));

        let signalled = state.cancel_background().unwrap();
        release_tx.send(()).unwrap();
        handle.join().unwrap().unwrap();
        (
            signalled,
            observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        )
    }

    #[test]
    fn cancel_background_signals_only_background_tasks() {
        // 用户操作不被后台让位请求误取消。
        assert_eq!(
            probe_background_cancel(BackupTaskKind::UserOperation),
            (false, false)
        );
        assert_eq!(
            probe_background_cancel(BackupTaskKind::AutomaticBackup),
            (true, true)
        );
        assert_eq!(
            probe_background_cancel(BackupTaskKind::IdleVerify),
            (true, true)
        );
    }

    #[test]
    fn cancel_background_is_a_noop_while_idle() {
        let state = BackupState::default();
        assert!(!state.cancel_background().unwrap());
        assert!(!state.cancelled());
    }

    #[test]
    fn request_cancel_still_stops_a_user_operation() {
        let state = BackupState::default();
        let (started_tx, started_rx) = mpsc::channel();
        let (observed_tx, observed_rx) = mpsc::channel();
        let running = state.clone();
        let handle = thread::spawn(move || {
            running.run_exclusive(None, BackupTaskKind::UserOperation, || {
                started_tx.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                while !running.cancelled() && Instant::now() < deadline {
                    thread::yield_now();
                }
                observed_tx.send(running.cancelled()).unwrap();
                Ok::<_, String>(())
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(state.request_cancel().unwrap());
        assert!(
            observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "用户主动取消必须对前台任务生效"
        );
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn foreground_wait_registers_and_releases_around_the_lock() {
        let state = BackupState::default();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = state.clone();
        let holding = thread::spawn(move || {
            holder.run_exclusive(None, BackupTaskKind::AutomaticBackup, || {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok::<_, String>(())
            })
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let waiter = state.clone();
        let waiting = thread::spawn(move || {
            let mut observed = None;
            waiter
                .run_foreground(None, || {
                    observed = Some(waiter.foreground_waiting());
                    Ok::<_, String>(())
                })
                .unwrap();
            observed
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while state.foreground_waiting() == 0 && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(state.foreground_waiting(), 1, "等锁期间必须计入前台等待");

        release_tx.send(()).unwrap();
        let observed_inside_lock = waiting.join().unwrap();
        holding.join().unwrap().unwrap();
        assert_eq!(state.foreground_waiting(), 0);
        assert_eq!(observed_inside_lock, Some(0), "取得运行锁后不再算作等待");
    }

    #[test]
    fn foreground_wait_is_released_on_failure_and_shutdown() {
        let state = BackupState::default();
        assert!(state
            .run_foreground(None, || Err::<(), _>("failed".to_string()))
            .is_err());
        assert_eq!(state.foreground_waiting(), 0);

        state.shutdown();
        assert!(state.run_foreground(None, || Ok::<_, String>(())).is_err());
        assert_eq!(state.foreground_waiting(), 0);
    }

    #[test]
    fn shutdown_waits_for_active_operation_and_rejects_queued_work() {
        let state = BackupState::default();
        let active_state = state.clone();
        let (active_started_tx, active_started_rx) = mpsc::channel();
        let (cancel_seen_tx, cancel_seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let active = thread::spawn(move || {
            active_state.run_exclusive(None, BackupTaskKind::AutomaticBackup, || {
                active_started_tx.send(()).unwrap();
                while !active_state.cancelled() {
                    thread::yield_now();
                }
                cancel_seen_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Err::<(), _>("cancelled".into())
            })
        });
        active_started_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();

        let queued_executed = Arc::new(AtomicBool::new(false));
        let queued_flag = queued_executed.clone();
        let queued_state = state.clone();
        let queued = thread::spawn(move || {
            queued_state.run_exclusive(None, BackupTaskKind::AutomaticBackup, || {
                queued_flag.store(true, Ordering::SeqCst);
                Ok(())
            })
        });

        let shutdown_state = state.clone();
        let (shutdown_done_tx, shutdown_done_rx) = mpsc::channel();
        let shutdown = thread::spawn(move || {
            shutdown_state.shutdown();
            shutdown_done_tx.send(()).unwrap();
        });
        cancel_seen_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let returned_before_cleanup = shutdown_done_rx
            .recv_timeout(Duration::from_millis(50))
            .is_ok();

        release_tx.send(()).unwrap();
        if !returned_before_cleanup {
            shutdown_done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
        }
        active.join().unwrap().unwrap_err();
        let queued_result = queued.join().unwrap();
        shutdown.join().unwrap();

        assert!(!returned_before_cleanup);
        assert!(queued_result.is_err());
        assert!(!queued_executed.load(Ordering::SeqCst));
    }
}
