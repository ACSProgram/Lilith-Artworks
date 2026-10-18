use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    app::AppState,
    backup::{BackupState, BackupTaskKind, ExclusiveRunError},
    repository_lock::{self, OwnerLockError, RepositoryOwnerLock},
    storage,
};

use super::{
    cancel::CancelToken,
    error::CommandError,
    events::{
        AppEvent, AppEventKind, NoopObserver, OperationObserver, OperationOutcome, SharedEventSink,
    },
    types::{OperationClass, OperationDomain, OperationId},
};

/// 一次仓库操作已被协调器分类并取得锁的证明。use-case 只能通过它拿到仓库根目录，
/// 避免在锁外重新读取设置里的仓库路径。
pub(crate) struct RepositorySession<'a> {
    root: &'a Path,
}

impl<'a> RepositorySession<'a> {
    fn new(root: &'a Path) -> Self {
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        self.root
    }
}

/// GUI、headless 与未来维护入口共用的运行入口。
///
/// 写操作的锁序固定为：跨进程仓库 owner lock → 共享运行锁（仅写/前台/后台分类）
/// → 仓库 lease → 仓库操作锁 → SQLite 事务。owner lock 冲突不排队，直接返回
/// `repository_busy`。只读命令（[`Self::run_read`]）与内容级只读长任务
/// （[`Self::run_foreground_read`]）不持有 owner lock。素材板写命令走
/// [`Self::run_repository_write`]，按既有语义只取仓库 lease 与操作锁，不取共享
/// 运行锁。
#[derive(Clone)]
pub(crate) struct OperationCoordinator {
    inner: Arc<Inner>,
}

struct Inner {
    app_state: AppState,
    backup_state: BackupState,
    events: SharedEventSink,
    observer: Arc<dyn OperationObserver>,
}

#[allow(dead_code)]
impl OperationCoordinator {
    pub(crate) fn new(
        app_state: AppState,
        backup_state: BackupState,
        events: SharedEventSink,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                app_state,
                backup_state,
                events,
                observer: Arc::new(NoopObserver),
            }),
        }
    }

    pub(crate) fn with_observer(self, observer: Arc<dyn OperationObserver>) -> Self {
        Self {
            inner: Arc::new(Inner {
                app_state: self.inner.app_state.clone(),
                backup_state: self.inner.backup_state.clone(),
                events: self.inner.events.clone(),
                observer,
            }),
        }
    }

    pub(crate) fn app_state(&self) -> &AppState {
        &self.inner.app_state
    }

    pub(crate) fn backup_state(&self) -> &BackupState {
        &self.inner.backup_state
    }

    /// 在阻塞线程上执行一个命令。GUI 适配器只调用这里，不再各自写
    /// `spawn_blocking` 与开始/结束日志。
    pub(crate) async fn spawn<T, F>(
        &self,
        domain: OperationDomain,
        operation: F,
    ) -> Result<T, CommandError>
    where
        T: Send + 'static,
        F: FnOnce(&CommandContext) -> Result<T, CommandError> + Send + 'static,
    {
        let coordinator = self.clone();
        match tauri::async_runtime::spawn_blocking(move || coordinator.execute(domain, operation))
            .await
        {
            Ok(result) => result,
            Err(error) => Err(CommandError::internal(format!("任务异常结束：{error}"))),
        }
    }

    /// 同步执行入口。headless 与单元测试直接调用它；GUI 的命令线程也只经
    /// [`Self::spawn`] 进入这里。
    pub(crate) fn execute<T>(
        &self,
        domain: OperationDomain,
        operation: impl FnOnce(&CommandContext) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let cancel = CancelToken::for_backup(self.inner.backup_state.clone());
        self.execute_with(domain, cancel, operation)
    }

    /// 允许调用方提供取消来源（无头闸门）或测试用的独立令牌。
    pub(crate) fn execute_with<T>(
        &self,
        domain: OperationDomain,
        cancel: CancelToken,
        operation: impl FnOnce(&CommandContext) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let operation_id = OperationId::new();
        let repository_key = self.repository_key();
        let started = Instant::now();
        let context = CommandContext {
            coordinator: self.clone(),
            operation_id: operation_id.clone(),
            domain,
            repository_key: repository_key.clone(),
            cancel,
            current_class: Mutex::new(OperationClass::Read),
            last_progress: Mutex::new(None),
        };

        let started_event = AppEvent::new(
            AppEventKind::OperationStarted,
            repository_key.clone(),
            Some(operation_id.as_str().to_owned()),
            Some(domain),
            true,
            self.completion_revision(),
        );
        self.inner.events.emit(&started_event);
        log::info!(
            "operation started: id={}, domain={}, repo={}",
            operation_id,
            domain.as_str(),
            repository_key.as_deref().unwrap_or("")
        );

        let result = operation(&context);
        let elapsed_ms = started.elapsed().as_millis();
        let revision = self.completion_revision();
        let (outcome, error) = match &result {
            Ok(_) => (OperationOutcome::Succeeded, None),
            Err(error) => {
                let outcome = if matches!(error.kind, super::error::CommandErrorKind::Cancelled) {
                    OperationOutcome::Cancelled
                } else {
                    OperationOutcome::Failed
                };
                (outcome, Some(error.summary()))
            }
        };
        let mut event = AppEvent::new(
            AppEventKind::OperationFinished { outcome, error },
            repository_key.clone(),
            Some(operation_id.as_str().to_owned()),
            Some(domain),
            true,
            revision,
        );
        event.phase = Some(outcome.as_str().to_owned());
        self.inner.events.emit(&event);

        match &result {
            Ok(_) => log::info!(
                "operation finished: id={}, domain={}, outcome={}, elapsed_ms={elapsed_ms}, repo={}",
                operation_id,
                domain.as_str(),
                outcome.as_str(),
                repository_key.as_deref().unwrap_or("")
            ),
            Err(error) => log::warn!(
                "operation failed: id={}, domain={}, outcome={}, elapsed_ms={elapsed_ms}, code={}, kind={}, retryable={}, error={}",
                operation_id,
                domain.as_str(),
                outcome.as_str(),
                error.code,
                error.kind.as_str(),
                error.retryable,
                error.message
            ),
        }
        result.map_err(|error| error.with_operation_id(operation_id.as_str()))
    }

    /// 用户写命令：共享运行锁（UserOperation）→ 仓库 lease → 仓库操作锁。
    pub(crate) fn run_write<T>(
        &self,
        context: &CommandContext,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.run_write_scoped(context, None, operation)
    }

    /// 与 [`Self::run_write`] 相同，但把命令归属到某个分支（运行状态里可见）。
    pub(crate) fn run_write_scoped<T>(
        &self,
        context: &CommandContext,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        context.set_class(OperationClass::Write);
        // 固定锁序的第一层：跨进程 owner lock。它在运行分类锁之前取得，冲突立即
        // 返回 repository_busy，不排队等待。
        let _owner = self.acquire_owner_lock(context, "write")?;
        let result = self.inner.backup_state.run_exclusive_typed(
            branch_id,
            BackupTaskKind::UserOperation,
            || {
                self.inner
                    .app_state
                    .with_ready_repository_typed(|root| operation(&RepositorySession::new(root)))
            },
        );
        self.finish_write(context, result)
    }

    /// 前台长命令：登记前台等待、请求后台让位，再取共享运行锁。
    pub(crate) fn run_foreground<T>(
        &self,
        context: &CommandContext,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        context.set_class(OperationClass::Foreground);
        let _owner = self.acquire_owner_lock(context, "foreground")?;
        let result = self.inner.backup_state.run_foreground_typed(branch_id, || {
            self.inner
                .app_state
                .with_ready_repository_typed(|root| operation(&RepositorySession::new(root)))
        });
        self.finish_write(context, result)
    }

    /// 内容级只读长任务（全库校验、画板 DDS 检查）：取得与前台命令相同的运行
    /// 锁与仓库操作锁以保证扫描目标不被本进程改写，但**不持有**跨进程 owner lock
    /// ——只读命令不得变成写者。开始前只探测一次：已有写者时立即以明确文案拒绝。
    /// 「探测后写者才开始」的窗口与本阶段之前的哨兵探测口径一致。
    pub(crate) fn run_foreground_read<T>(
        &self,
        context: &CommandContext,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        context.set_class(OperationClass::Foreground);
        self.probe_repository_owner(context)?;
        let result = self.inner.backup_state.run_foreground_typed(branch_id, || {
            self.inner
                .app_state
                .with_ready_repository_typed(|root| operation(&RepositorySession::new(root)))
        });
        match result {
            Ok(value) => Ok(value),
            Err(ExclusiveRunError::ShuttingDown) => Err(CommandError::shutting_down()),
            Err(ExclusiveRunError::State(message)) => Err(CommandError::internal(message)),
            Err(ExclusiveRunError::Operation(error)) => Err(error),
        }
    }

    /// 后台任务：共享运行锁（AutomaticBackup）。阶段 5 的调度器会复用它。
    pub(crate) fn run_background<T>(
        &self,
        context: &CommandContext,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        context.set_class(OperationClass::Background);
        let _owner = self.acquire_owner_lock(context, "background")?;
        let result = self.inner.backup_state.run_exclusive_typed(
            branch_id,
            BackupTaskKind::AutomaticBackup,
            || {
                self.inner
                    .app_state
                    .with_ready_repository_typed(|root| operation(&RepositorySession::new(root)))
            },
        );
        self.finish_write(context, result)
    }

    /// 素材板等既有写命令：只取仓库 lease + 仓库操作锁，不取共享运行锁。
    pub(crate) fn run_repository_write<T>(
        &self,
        context: &CommandContext,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        context.set_class(OperationClass::RepositoryWrite);
        let _owner = self.acquire_owner_lock(context, "repositoryWrite")?;
        let result = self
            .inner
            .app_state
            .with_ready_repository_typed(|root| operation(&RepositorySession::new(root)));
        if result.is_ok() {
            self.emit_repository_changed(context);
        }
        result
    }

    /// 设置保存 / 仓库切换：先取运行分类锁，再取 AppState 的写租约。保存设置
    /// 需要同时替换仓库路径与设置文件，不能先持读 lease。
    pub(crate) fn run_repository_switch<T>(
        &self,
        context: &CommandContext,
        target_repository: Option<&Path>,
        foreground: bool,
        operation: impl FnOnce() -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        context.set_class(if foreground {
            OperationClass::Foreground
        } else {
            OperationClass::Write
        });
        // 切换/首次配置仓库时锁的是目标仓库；清空仓库路径则没有需要保护的写者。
        let _owner = target_repository
            .map(|root| self.acquire_owner_for_root(root, "settings:switch"))
            .transpose()?;
        let result = if foreground {
            self.inner.backup_state.run_foreground_typed(None, || {
                self.inner.app_state.with_repository_switch_typed(operation)
            })
        } else {
            self.inner
                .backup_state
                .run_exclusive_typed(None, BackupTaskKind::UserOperation, || {
                    self.inner.app_state.with_repository_switch_typed(operation)
                })
        };
        self.finish_write(context, result)
    }

    /// 对当前仓库取得跨进程 owner lock。尚未配置仓库或路径尚未建立时返回 `None`，
    /// 由后续的仓库校验给出规范错误；只读命令不调用这里。
    fn acquire_owner_lock(
        &self,
        context: &CommandContext,
        action: &str,
    ) -> Result<Option<RepositoryOwnerLock>, CommandError> {
        let Some(root) = context.repository_path()? else {
            return Ok(None);
        };
        if !root.is_dir() {
            return Ok(None);
        }
        let operation = format!("{}:{action}", context.domain().as_str());
        self.acquire_owner_for_root(&root, &operation).map(Some)
    }

    fn probe_repository_owner(&self, context: &CommandContext) -> Result<(), CommandError> {
        let Some(root) = context.repository_path()? else {
            return Ok(());
        };
        if !root.is_dir() {
            return Ok(());
        }
        repository_lock::probe(&root).map_err(CommandError::from)
    }

    fn acquire_owner_for_root(
        &self,
        root: &Path,
        operation: &str,
    ) -> Result<RepositoryOwnerLock, CommandError> {
        repository_lock::RepositoryOwnerLock::acquire(
            root,
            self.inner.app_state.process_role(),
            operation,
        )
        .map_err(owner_lock_error)
    }

    fn finish_write<T>(
        &self,
        context: &CommandContext,
        result: Result<T, ExclusiveRunError<CommandError>>,
    ) -> Result<T, CommandError> {
        match result {
            Ok(value) => {
                self.emit_repository_changed(context);
                Ok(value)
            }
            Err(ExclusiveRunError::ShuttingDown) => Err(CommandError::shutting_down()),
            Err(ExclusiveRunError::State(message)) => Err(CommandError::internal(message)),
            Err(ExclusiveRunError::Operation(error)) => Err(error),
        }
    }

    pub(crate) fn emit_repository_changed(&self, context: &CommandContext) {
        let mut event = AppEvent::new(
            AppEventKind::RepositoryChanged,
            context.repository_key.clone(),
            Some(context.operation_id.as_str().to_owned()),
            Some(context.domain),
            false,
            self.completion_revision(),
        );
        event.phase = Some("repositoryChanged".to_owned());
        self.inner.events.emit(&event);
    }

    pub(crate) fn emit_cleanup_queue_changed(
        &self,
        context: &CommandContext,
        pending_count: Option<u64>,
    ) {
        let mut event = AppEvent::new(
            AppEventKind::CleanupQueueChanged { pending_count },
            context.repository_key.clone(),
            Some(context.operation_id.as_str().to_owned()),
            Some(context.domain),
            false,
            self.completion_revision(),
        );
        event.phase = Some("cleanupQueueChanged".to_owned());
        self.inner.events.emit(&event);
    }

    pub(crate) fn emit_progress(
        &self,
        context: &CommandContext,
        label: &str,
        current: u64,
        total: u64,
    ) {
        self.inner.observer.progress(label, current, total);
        // 事件通道只表达可消费的进度：阶段变化、完成、回退或间隔足够长时才广播。
        // 观察者（无头 marker）不受节流影响，仍能看到每一次检查点。
        const PROGRESS_EVENT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
        let should_emit = match context.last_progress.lock() {
            Ok(mut last) => {
                let now = Instant::now();
                let emit = match last.as_ref() {
                    Some((at, last_label, last_current)) => {
                        last_label != label
                            || current == total
                            || current < *last_current
                            || now.duration_since(*at) >= PROGRESS_EVENT_INTERVAL
                    }
                    None => true,
                };
                if emit {
                    *last = Some((now, label.to_owned(), current));
                }
                emit
            }
            Err(_) => true,
        };
        if !should_emit {
            return;
        }
        let mut event = AppEvent::new(
            AppEventKind::OperationProgress,
            context.repository_key.clone(),
            Some(context.operation_id.as_str().to_owned()),
            Some(context.domain),
            true,
            self.completion_revision(),
        );
        event.phase = Some(label.to_owned());
        event.current = Some(current);
        event.total = Some(total);
        self.inner.events.emit(&event);
    }

    pub(crate) fn report_runtime_progress(
        &self,
        context: &CommandContext,
        label: &str,
        current: u64,
        total: u64,
    ) {
        let class = context.class();
        if !matches!(
            class,
            OperationClass::Read | OperationClass::RepositoryWrite
        ) {
            log::debug!(
                "operation progress: class={}, label={label}, current={current}, total={total}",
                class.as_str()
            );
            self.inner
                .backup_state
                .report_progress(context.domain.as_str(), label, current, total);
        }
    }

    pub(crate) fn completion_revision(&self) -> u64 {
        self.inner
            .backup_state
            .status()
            .map(|status| status.completion_revision)
            .unwrap_or(0)
    }

    fn repository_key(&self) -> Option<String> {
        self.inner
            .app_state
            .repository_path()
            .ok()
            .flatten()
            .map(|path| storage::display_path(&path))
    }
}

fn owner_lock_error(error: OwnerLockError) -> CommandError {
    match &error {
        OwnerLockError::Busy(_) => CommandError::busy(error.user_message()),
        OwnerLockError::Io(_) => CommandError::from_legacy(error.user_message()),
    }
}

pub(crate) struct CommandContext {
    coordinator: OperationCoordinator,
    operation_id: OperationId,
    domain: OperationDomain,
    repository_key: Option<String>,
    cancel: CancelToken,
    current_class: Mutex<OperationClass>,
    last_progress: Mutex<Option<(Instant, String, u64)>>,
}

#[allow(dead_code)]
impl CommandContext {
    pub(crate) fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    pub(crate) fn domain(&self) -> OperationDomain {
        self.domain
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub(crate) fn app_state(&self) -> &AppState {
        self.coordinator.app_state()
    }

    pub(crate) fn backup_state(&self) -> &BackupState {
        self.coordinator.backup_state()
    }

    pub(crate) fn repository_path(&self) -> Result<Option<PathBuf>, CommandError> {
        self.app_state()
            .repository_path()
            .map_err(CommandError::from)
    }

    pub(crate) fn run_read<T>(
        &self,
        label: &str,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.set_class(OperationClass::Read);
        self.coordinator
            .inner
            .app_state
            .with_repository_read_labeled_typed(label, |root| {
                operation(&RepositorySession::new(root))
            })
    }

    fn set_class(&self, class: OperationClass) {
        if let Ok(mut current) = self.current_class.lock() {
            *current = class;
        }
    }

    fn class(&self) -> OperationClass {
        self.current_class
            .lock()
            .map(|class| *class)
            .unwrap_or(OperationClass::Read)
    }

    pub(crate) fn run_write<T>(
        &self,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator.run_write(self, operation)
    }

    pub(crate) fn run_write_scoped<T>(
        &self,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator
            .run_write_scoped(self, branch_id, operation)
    }

    pub(crate) fn run_foreground<T>(
        &self,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator.run_foreground(self, branch_id, operation)
    }

    pub(crate) fn run_foreground_read<T>(
        &self,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator
            .run_foreground_read(self, branch_id, operation)
    }

    pub(crate) fn run_background<T>(
        &self,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator.run_background(self, branch_id, operation)
    }

    pub(crate) fn run_repository_write<T>(
        &self,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator.run_repository_write(self, operation)
    }

    pub(crate) fn run_repository_switch<T>(
        &self,
        target_repository: Option<&Path>,
        foreground: bool,
        operation: impl FnOnce() -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator
            .run_repository_switch(self, target_repository, foreground, operation)
    }

    pub(crate) fn report_progress(&self, label: &str, current: u64, total: u64) {
        self.coordinator.emit_progress(self, label, current, total);
        self.coordinator
            .report_runtime_progress(self, label, current, total);
    }

    pub(crate) fn emit_repository_changed(&self) {
        self.coordinator.emit_repository_changed(self);
    }

    pub(crate) fn emit_cleanup_queue_changed(&self, pending_count: Option<u64>) {
        self.coordinator
            .emit_cleanup_queue_changed(self, pending_count);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc, Arc, Mutex,
        },
        thread,
        time::Duration,
    };

    use super::*;
    use crate::app::command_runtime::events::{AppEventKind, EventSink, NoopEventSink};

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<AppEvent>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event: &AppEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    fn test_coordinator() -> (OperationCoordinator, Arc<RecordingSink>) {
        let directory = tempfile::tempdir().unwrap();
        let app_state = AppState::new(
            Default::default(),
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        // 泄漏临时目录：测试只关心事件与锁行为，不依赖目录被清理。
        std::mem::forget(directory);
        let backup_state = BackupState::default();
        let sink = Arc::new(RecordingSink::default());
        (
            OperationCoordinator::new(app_state, backup_state, sink.clone()),
            sink,
        )
    }

    #[test]
    fn operations_emit_started_and_finished_with_stable_fields() {
        let (coordinator, sink) = test_coordinator();
        let result = coordinator.execute(OperationDomain::Library, |context| {
            context.report_progress("读取作品树", 1, 3);
            Ok::<_, CommandError>(7)
        });
        assert_eq!(result.unwrap(), 7);

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0].kind, AppEventKind::OperationStarted));
        assert!(matches!(events[1].kind, AppEventKind::OperationProgress));
        assert_eq!(events[1].phase.as_deref(), Some("读取作品树"));
        assert_eq!(events[1].current, Some(1));
        assert_eq!(events[1].total, Some(3));
        assert!(matches!(
            events[2].kind,
            AppEventKind::OperationFinished {
                outcome: OperationOutcome::Succeeded,
                ..
            }
        ));
        assert_eq!(events[0].version, 1);
        assert_eq!(events[0].operation_id, events[2].operation_id);
    }

    #[test]
    fn failed_operations_carry_code_and_kind() {
        let (coordinator, sink) = test_coordinator();
        let error = coordinator
            .execute(OperationDomain::Backup, |_| {
                Err::<(), _>(CommandError::busy("仓库正忙"))
            })
            .unwrap_err();
        assert_eq!(error.code, "repository_busy");
        assert!(error.operation_id.is_some());

        let events = sink.events.lock().unwrap();
        match &events.last().unwrap().kind {
            AppEventKind::OperationFinished { outcome, error } => {
                assert_eq!(*outcome, OperationOutcome::Failed);
                let summary = error.as_ref().unwrap();
                assert_eq!(summary.code, "repository_busy");
                assert!(summary.retryable);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn a_cancelled_token_is_visible_inside_the_operation() {
        let (coordinator, _sink) = test_coordinator();
        let token = CancelToken::standalone();
        token.cancel();
        let seen = coordinator
            .execute_with(OperationDomain::Cleanup, token, |context| {
                Ok::<_, CommandError>(context.is_cancelled())
            })
            .unwrap();
        assert!(seen);
    }

    #[test]
    fn a_write_after_shutdown_is_rejected_with_the_typed_shutdown_code() {
        let (coordinator, _sink) = test_coordinator();
        coordinator.backup_state().shutdown();
        let error = coordinator
            .execute(OperationDomain::Library, |context| {
                context.run_write(|_session| Ok::<_, CommandError>(()))
            })
            .unwrap_err();
        assert_eq!(error.code, "shutting_down");
        assert_eq!(
            error.kind,
            crate::app::command_runtime::CommandErrorKind::Cancelled
        );
    }

    #[test]
    fn a_second_process_cannot_write_the_same_repository() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();

        let coordinator_for = |name: &str| {
            let mut settings = crate::app::settings::AppSettings::default();
            settings.repository_path = root.to_string_lossy().into_owned();
            let state = AppState::new(
                settings,
                directory.path().join(format!("{name}.json")),
                directory.path().join(format!("{name}-logs")),
                None,
            );
            OperationCoordinator::new(state, BackupState::default(), Arc::new(NoopEventSink))
        };

        let first = coordinator_for("first");
        let second = coordinator_for("second");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holding = thread::spawn(move || {
            first
                .execute(OperationDomain::Library, |context| {
                    context.run_write(|_| {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok::<_, CommandError>(())
                    })
                })
                .unwrap();
        });
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("第一个写者未取得 owner lock");

        let error = second
            .execute(OperationDomain::Library, |context| {
                context.run_write(|_| Ok::<_, CommandError>(()))
            })
            .unwrap_err();
        assert_eq!(error.code, "repository_busy");
        assert!(
            error.message.contains("另一个进程写入"),
            "{}",
            error.message
        );

        release_tx.send(()).unwrap();
        holding.join().unwrap();
        second
            .execute(OperationDomain::Library, |context| {
                context.run_write(|_| Ok::<_, CommandError>(()))
            })
            .expect("释放后第二个写者应能继续");
    }

    #[test]
    fn repository_write_does_not_take_the_shared_runtime_lock() {
        let (coordinator, sink) = test_coordinator();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = coordinator.clone();
        let holding = thread::spawn(move || {
            holder
                .backup_state()
                .run_exclusive(None, BackupTaskKind::AutomaticBackup, || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                    Ok::<_, String>(())
                })
                .unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // 运行锁被后台任务持有时，素材板式仓库写仍能推进（只等仓库操作锁）。
        let completed = Arc::new(AtomicUsize::new(0));
        let flag = completed.clone();
        let result = coordinator.execute(OperationDomain::PinBoard, |context| {
            context.run_repository_write(|session| {
                flag.fetch_add(1, Ordering::SeqCst);
                Ok::<_, CommandError>(session.root().to_path_buf())
            })
        });
        // 未配置仓库，所以这里失败；关键是它没有等待运行锁并在 2 秒内返回。
        assert!(result.is_err());
        assert_eq!(completed.load(Ordering::SeqCst), 0);

        release_tx.send(()).unwrap();
        holding.join().unwrap();
        let events = sink.events.lock().unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event.kind, AppEventKind::OperationFinished { .. })));
    }
}
