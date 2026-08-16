use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

use tauri::AppHandle;

use super::BackupRuntimeStatus;

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
    pub(crate) fn run_exclusive<T>(
        &self,
        branch_id: Option<&str>,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.run_exclusive_typed(branch_id, operation)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn run_exclusive_typed<T, E>(
        &self,
        branch_id: Option<&str>,
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
        self.inner.cancel_requested.store(false, Ordering::SeqCst);
        {
            let mut runtime = self
                .inner
                .runtime
                .lock()
                .map_err(|_| ExclusiveRunError::State("备份状态已损坏".into()))?;
            runtime.busy = true;
            runtime.active_branch_id = branch_id.map(str::to_owned);
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

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        },
        thread,
        time::Duration,
    };

    use super::*;

    #[test]
    fn completion_revision_advances_for_short_operations() {
        let state = BackupState::default();
        assert_eq!(state.status().unwrap().completion_revision, 0);

        state.run_exclusive(None, || Ok::<_, String>(())).unwrap();
        let first = state.status().unwrap();
        assert!(!first.busy);
        assert_eq!(first.completion_revision, 1);

        state
            .run_exclusive(None, || Err::<(), _>("failed".to_string()))
            .unwrap_err();
        assert_eq!(state.status().unwrap().completion_revision, 2);
    }

    #[test]
    fn shutdown_waits_for_active_operation_and_rejects_queued_work() {
        let state = BackupState::default();
        let active_state = state.clone();
        let (active_started_tx, active_started_rx) = mpsc::channel();
        let (cancel_seen_tx, cancel_seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let active = thread::spawn(move || {
            active_state.run_exclusive(None, || {
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
            queued_state.run_exclusive(None, || {
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
