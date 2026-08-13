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

#[derive(Clone, Default)]
pub(crate) struct BackupState {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    operation_lock: Mutex<()>,
    runtime: Mutex<BackupRuntimeStatus>,
    cancel_requested: AtomicBool,
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
        let _guard = self
            .inner
            .operation_lock
            .lock()
            .map_err(|_| "备份操作锁已损坏")?;
        self.inner.cancel_requested.store(false, Ordering::SeqCst);
        {
            let mut runtime = self.inner.runtime.lock().map_err(|_| "备份状态已损坏")?;
            runtime.busy = true;
            runtime.active_branch_id = branch_id.map(str::to_owned);
        }
        let result = operation();
        if let Ok(mut runtime) = self.inner.runtime.lock() {
            let scheduling = runtime.automatic_scheduling;
            *runtime = BackupRuntimeStatus {
                automatic_scheduling: scheduling,
                ..Default::default()
            };
        }
        self.inner.cancel_requested.store(false, Ordering::SeqCst);
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
        if let Ok(mut runtime) = self.inner.runtime.lock() {
            runtime.automatic_scheduling = false;
        }
    }
}
