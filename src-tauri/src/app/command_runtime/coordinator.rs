use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    app::AppState,
    backup::{BackupState, BackupTaskKind, ExclusiveRunError},
    cancel::CancelToken,
    platform::path::display_path,
    repository_lock::{
        self, OwnerLockError, RepositoryOwnerLock, WaitPolicy, WritePriority,
        WRITE_WAIT_TIMEOUT_MESSAGE,
    },
};

use super::{
    error::CommandError,
    events::{
        AppEvent, AppEventKind, NoopObserver, OperationObserver, OperationOutcome, SharedEventSink,
    },
    types::{OperationClass, OperationDomain, OperationId},
};

/// 交互写（普通写命令、素材板写命令）排队取闸门的上限。
///
/// 依据：实测最长持锁 40,234 ms；但交互写会先请求后台让位（`cancel_background`），
/// 自动备份通常在一个分块边界内退出，因此 10 秒足够覆盖正常情况，超时即按
/// 「仓库正在写入，请稍后重试」快速失败，而不是让按钮无限转圈。
const INTERACTIVE_WRITE_WAIT: Duration = Duration::from_secs(10);
/// 前台长命令排队上限：恢复/精简/检查点/整仓备份这类操作必然与自动备份重叠，
/// 而自动备份实测最长持锁 40 秒，留出余量。
const FOREGROUND_WRITE_WAIT: Duration = Duration::from_secs(60);
/// 切换仓库排队上限：切换本身还要等运行锁与写租约，闸门只占其中一小段。
const REPOSITORY_SWITCH_WRITE_WAIT: Duration = Duration::from_secs(30);
/// 退出结算排队上限：原生端有 15 秒兜底强退，这里留 5 秒余量。
const SHUTDOWN_WRITE_WAIT: Duration = Duration::from_secs(10);

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
/// 写操作的锁序固定为：**进程内写入闸门（排队）** → 跨进程仓库 owner lock（零等待）
/// → 共享运行锁（仅写/前台分类）→ 仓库 lease → 仓库操作锁 → SQLite 事务。
///
/// 两层判定的分工：
///
/// - 闸门只处理**同进程**竞争：按 Shutdown > Foreground > Interactive > Background
///   串行排队，等待有上限且可取消，因此同进程的两条写路径不再互相返回「另一个进程
///   正在写入」。
/// - 跨进程 owner lock 的语义**完全不变**：另一个进程持锁时仍然立即拒绝；排队成功
///   之后才发现 SQLite 被别的进程占用时，闸门随之释放并返回原样文案。
///
/// 只读命令（[`Self::run_read`]）与内容级只读长任务（[`Self::run_foreground_read`]）
/// 不持有 owner lock。素材板写命令走 [`Self::run_repository_write`]，按既有语义只取
/// 仓库 lease 与操作锁，不取共享运行锁。
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

    #[cfg_attr(not(feature = "headless"), allow(dead_code))] // headless 运行时用它接收进度。
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
            last_progress_log: Mutex::new(None),
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
            AppEventKind::OperationFinished {
                outcome,
                error,
                artwork_id: None,
                structure_changed: None,
            },
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

    /// 用户写命令：写入闸门（Interactive，排队）→ 共享运行锁（UserOperation）
    /// → 仓库 lease → 仓库操作锁。
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
        let _activity = UserActivity::begin(&self.inner.app_state);
        context.set_class(OperationClass::Write);
        // 固定锁序的第一层：进程内闸门（排队）与跨进程 owner lock（零等待）。
        let _owner = self.acquire_interactive_owner(context, "write")?;
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

    /// 前台长命令：登记前台等待 + 请求后台让位，再取写入闸门（Foreground）与
    /// 共享运行锁。
    ///
    /// 让位必须发生在取闸门**之前**：闸门一失败就直接返回，根本走不到让位那一步；
    /// 这也是 0.2 → 0.3 回归的原因。
    pub(crate) fn run_foreground<T>(
        &self,
        context: &CommandContext,
        branch_id: Option<&str>,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let _activity = UserActivity::begin(&self.inner.app_state);
        context.set_class(OperationClass::Foreground);
        let mut waiting = self.inner.backup_state.begin_foreground_wait();
        let _ = self.inner.backup_state.cancel_background();
        let _owner = self.acquire_owner_lock(
            context,
            "foreground",
            WritePriority::Foreground,
            WaitPolicy::Until(Instant::now() + FOREGROUND_WRITE_WAIT),
            &self.gate_cancel(),
        )?;
        let result =
            self.inner
                .backup_state
                .run_foreground_after_wait(branch_id, &mut waiting, || {
                    self.inner.app_state.with_ready_repository_typed(|root| {
                        operation(&RepositorySession::new(root))
                    })
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
        let _activity = UserActivity::begin(&self.inner.app_state);
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

    /// 素材板等既有写命令：写入闸门（Interactive，排队）+ 跨进程 owner lock，
    /// 之后只取仓库 lease + 仓库操作锁，不取共享运行锁。
    pub(crate) fn run_repository_write<T>(
        &self,
        context: &CommandContext,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let _activity = UserActivity::begin(&self.inner.app_state);
        context.set_class(OperationClass::RepositoryWrite);
        let _owner = self.acquire_interactive_owner(context, "repositoryWrite")?;
        let result = self
            .inner
            .app_state
            .with_ready_repository_typed(|root| operation(&RepositorySession::new(root)));
        if result.is_ok() {
            self.emit_repository_changed(context);
        }
        result
    }

    /// 退出结算专用的素材板写命令：用 Shutdown 优先级插队，并把等待上限压到
    /// [`SHUTDOWN_WRITE_WAIT`]，保证关窗流程不会拖过原生端的 15 秒兜底强退。
    pub(crate) fn run_repository_write_shutdown<T>(
        &self,
        context: &CommandContext,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let _activity = UserActivity::begin(&self.inner.app_state);
        context.set_class(OperationClass::RepositoryWrite);
        let _owner = self.acquire_owner_lock(
            context,
            "repositoryWrite",
            WritePriority::Shutdown,
            WaitPolicy::Until(Instant::now() + SHUTDOWN_WRITE_WAIT),
            // 退出结算不能用「观察退出」的取消来源：它要做的正是退出流程的最后一步。
            &CancelToken::standalone(),
        )?;
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
        let _activity = UserActivity::begin(&self.inner.app_state);
        context.set_class(if foreground {
            OperationClass::Foreground
        } else {
            OperationClass::Write
        });
        // 切换仓库本来就带前台语义：切换期间让后台让位并排队到上限，避免用户在
        // 自动备份期间切仓库时被一条「仓库正在写入」挡住。
        let (priority, wait) = if foreground {
            (
                WritePriority::Foreground,
                WaitPolicy::Until(Instant::now() + REPOSITORY_SWITCH_WRITE_WAIT),
            )
        } else {
            (
                WritePriority::Interactive,
                WaitPolicy::Until(Instant::now() + INTERACTIVE_WRITE_WAIT),
            )
        };
        // 切换/首次配置仓库时锁的是目标仓库；清空仓库路径则没有需要保护的写者。
        let _owner = target_repository
            .map(|root| {
                self.acquire_owner_for_root(
                    root,
                    "settings:switch",
                    priority,
                    wait,
                    &self.gate_cancel(),
                )
                .map_err(owner_lock_error)
            })
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

    /// 对当前仓库取得跨进程 owner lock（先排队通过进程内闸门）。尚未配置仓库或
    /// 路径尚未建立时返回 `None`，由后续的仓库校验给出规范错误；只读命令不调用这里。
    fn acquire_owner_lock(
        &self,
        context: &CommandContext,
        action: &str,
        priority: WritePriority,
        wait: WaitPolicy,
        cancel: &CancelToken,
    ) -> Result<Option<RepositoryOwnerLock>, CommandError> {
        let Some(root) = context.repository_path()? else {
            return Ok(None);
        };
        if !root.is_dir() {
            return Ok(None);
        }
        let operation = format!("{}:{action}", context.domain().as_str());
        self.acquire_owner_for_root(&root, &operation, priority, wait, cancel)
            .map(Some)
            .map_err(owner_lock_error)
    }

    /// 交互写的两段式取锁：先 `Try`，只有真的与本进程写者竞争时才请求后台让位并
    /// 排队到上限。这样每次画板保存都不会无谓地广播取消请求。
    fn acquire_interactive_owner(
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
        match self.acquire_owner_for_root(
            &root,
            &operation,
            WritePriority::Interactive,
            WaitPolicy::Try,
            &CancelToken::standalone(),
        ) {
            Ok(lock) => Ok(Some(lock)),
            // 闸门被本进程的写者占用：先请后台让位，再排队到上限。
            Err(OwnerLockError::InProcessBusy) => {
                let _ = self.inner.backup_state.cancel_background();
                self.acquire_owner_for_root(
                    &root,
                    &operation,
                    WritePriority::Interactive,
                    WaitPolicy::Until(Instant::now() + INTERACTIVE_WRITE_WAIT),
                    &self.gate_cancel(),
                )
                .map(Some)
                .map_err(owner_lock_error)
            }
            // 跨进程占用仍然立即拒绝：排队等不到另一个进程释放。
            Err(error) => Err(owner_lock_error(error)),
        }
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

    /// 闸门排队的取消来源：只观察应用退出。
    ///
    /// 不能复用 `context.cancel()`：它读的是「本进程当前是否有串行任务被取消」，
    /// 而前台/交互写为了拿到闸门会主动 `cancel_background()` 取消正在运行的自动
    /// 备份——那会让排队中的自己在下一刻就读到「已取消」。队列等待的真正上限是
    /// 截止时刻；退出流程则用这个来源提前结束等待。
    fn gate_cancel(&self) -> CancelToken {
        let backup_state = self.inner.backup_state.clone();
        CancelToken::with_source(move || backup_state.shutting_down())
    }

    fn acquire_owner_for_root(
        &self,
        root: &Path,
        operation: &str,
        priority: WritePriority,
        wait: WaitPolicy,
        cancel: &CancelToken,
    ) -> Result<RepositoryOwnerLock, OwnerLockError> {
        // `Try` 的语义在 `repository_lock` 里只有一份实现；排队路径才需要
        // 截止时刻与取消来源。
        if matches!(wait, WaitPolicy::Try) {
            return repository_lock::try_acquire(
                root,
                self.inner.app_state.process_role(),
                operation,
                priority,
            );
        }
        RepositoryOwnerLock::acquire_queued(
            root,
            self.inner.app_state.process_role(),
            operation,
            priority,
            wait,
            cancel,
        )
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
        // 进度值按累计口径上报，因此同一阶段内 `current` 单调不减，
        // `current < last_current` 只会在同名阶段重新开始时出现。
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
            // 进度回调在大文件上是逐块触发的（一次恢复可达十万次），逐次写日志
            // 会自己把日志文件刷爆，也让真正的事件淹没在噪声里。这里只按阶段
            // 切换和固定间隔抽样，慢操作仍然留下可读的时间线。
            const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);
            let should_log = match context.last_progress_log.lock() {
                Ok(mut last) => {
                    let now = Instant::now();
                    let log = match last.as_ref() {
                        Some((at, last_label)) => {
                            last_label != label || now.duration_since(*at) >= PROGRESS_LOG_INTERVAL
                        }
                        None => true,
                    };
                    if log {
                        *last = Some((now, label.to_owned()));
                    }
                    log
                }
                Err(_) => false,
            };
            if should_log {
                log::debug!(
                    "operation progress: class={}, label={label}, current={current}, total={total}",
                    class.as_str()
                );
            }
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
            .map(|path| display_path(&path))
    }
}

/// owner lock 失败到命令错误的映射。
///
/// - 跨进程占用：保留 0.2 的文案与 `repository_busy`。
/// - 本进程排队超时/占位：仍是 `repository_busy`（可重试），但文案改成
///   「仓库正在写入，请稍后重试」——同进程竞争时不能说「另一个进程」。
/// - 排队被取消：走统一的取消分类，不进入业务闭包。
fn owner_lock_error(error: OwnerLockError) -> CommandError {
    match &error {
        OwnerLockError::Busy(_) => CommandError::busy(error.user_message()),
        OwnerLockError::InProcessBusy | OwnerLockError::TimedOut => {
            CommandError::busy(WRITE_WAIT_TIMEOUT_MESSAGE)
        }
        OwnerLockError::Cancelled => CommandError::cancelled(error.user_message()),
        OwnerLockError::Io(_) => CommandError::from_legacy(error.user_message()),
    }
}

/// 用户长操作的活动期守卫：写入命令与内容级只读长任务（全库校验、画板检查）
/// 都算「用户正在动这个仓库」。
///
/// 进入时刷新一次活动时间，退出时再刷新一次，使后台静默窗口从命令真正结束的
/// 时刻重新计时。调度器按剩余静默期挂起等待，因此这里不逐次唤醒它——素材板
/// 之类的频繁保存不会反复触发候选扫描。
struct UserActivity {
    app_state: AppState,
}

impl UserActivity {
    fn begin(app_state: &AppState) -> Self {
        app_state.note_user_activity();
        Self {
            app_state: app_state.clone(),
        }
    }
}

impl Drop for UserActivity {
    fn drop(&mut self) {
        self.app_state.note_user_activity();
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
    /// 进度日志的抽样状态：阶段名与上次写日志的时刻。进度回调在热路径上
    /// 逐块触发，日志只按阶段切换或固定间隔抽样（见 `docs/guides/logging.md`）。
    last_progress_log: Mutex<Option<(Instant, String)>>,
}

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

    /// The shared cancellation token for long I/O. Stage 4 chunk-file APIs take
    /// this token directly so every read/write step observes the same source.
    pub(crate) fn cancel(&self) -> &CancelToken {
        &self.cancel
    }

    pub(crate) fn chunk_write_options(&self) -> crate::backup::chunk_file::ChunkWriteOptions {
        self.app_state().chunk_write_options()
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

    pub(crate) fn run_repository_write<T>(
        &self,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator.run_repository_write(self, operation)
    }

    /// 退出结算专用入口：Shutdown 优先级插队，等待上限 10 秒。
    pub(crate) fn run_repository_write_shutdown<T>(
        &self,
        operation: impl FnOnce(&RepositorySession) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator
            .run_repository_write_shutdown(self, operation)
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
        assert!(events.iter().all(|event| match &event.kind {
            AppEventKind::OperationFinished {
                artwork_id,
                structure_changed,
                ..
            } => artwork_id.is_none() && structure_changed.is_none(),
            _ => true,
        }));
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
            AppEventKind::OperationFinished { outcome, error, .. } => {
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

    /// 同进程的两个写者（不同 `AppState`、同一仓库）不再互相拒绝：第二个写者在
    /// 写入闸门上排队，等第一个闭包退出后进入。
    ///
    /// 真正的跨进程拒绝由 `repository_lock` 的单测与压力测试 I1/I2 覆盖。
    #[test]
    fn a_second_writer_queues_behind_the_first() {
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
        let (first_done_tx, first_done_rx) = mpsc::channel();
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
            first_done_tx.send(()).unwrap();
        });
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("第一个写者未取得仓库锁");

        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let queued = thread::spawn(move || {
            let result = second.execute(OperationDomain::Library, |context| {
                context.run_write(|_| {
                    second_entered_tx.send(()).unwrap();
                    Ok::<_, CommandError>(())
                })
            });
            result.map_err(|error| (error.code, error.message))
        });

        // 排队中的写者不得抢在第一个闭包退出之前进入。
        assert!(
            second_entered_rx
                .recv_timeout(Duration::from_millis(500))
                .is_err(),
            "第二个写者不得在第一个写者退出前进入"
        );

        release_tx.send(()).unwrap();
        first_done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("第一个写者未按时退出");
        // 第一个释放后第二个应在排队等待的剩余预算内成功进入。
        second_entered_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("释放后第二个写者应立即进入");
        assert!(
            queued.join().unwrap().is_ok(),
            "同进程写者必须排队而不是被拒绝"
        );
        holding.join().unwrap();
    }

    /// 交互写申请让位的代价：只有在真有进程内竞争时才请求后台取消。
    #[test]
    fn an_uncontended_interactive_write_does_not_cancel_background_work() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let mut settings = crate::app::settings::AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();

        let backup_state = BackupState::default();
        let state = AppState::new(
            settings,
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        let coordinator =
            OperationCoordinator::new(state, backup_state.clone(), Arc::new(NoopEventSink));
        // 登记一个后台任务，用于观察是否收到了取消请求。
        let background = backup_state.begin_task(BackupTaskKind::AutomaticBackup, None);

        coordinator
            .execute(OperationDomain::Library, |context| {
                context.run_write(|_| Ok::<_, CommandError>(()))
            })
            .expect("无竞争时写命令必须成功");

        assert!(
            !background.cancel_token().is_cancelled(),
            "没有闸门竞争时不应请求后台让位"
        );
        background.complete();
    }

    /// 排队中的交互写必须活过自己的「后台让位」请求。
    ///
    /// 闸门等待若复用命令的取消令牌（读「本进程是否有串行任务被取消」），让位请求
    /// 取消自动备份的那一刻就会把自己判成已取消，整个排队修复随之失效。
    #[test]
    fn a_queued_interactive_write_survives_the_background_yield_request() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let mut settings = crate::app::settings::AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();
        let backup_state = BackupState::default();
        let state = AppState::new(
            settings,
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        let coordinator =
            OperationCoordinator::new(state, backup_state.clone(), Arc::new(NoopEventSink));

        // 自动备份已登记任务并正持闸门（模拟跑到一半、还没到分块边界）。
        let background = backup_state.begin_task(BackupTaskKind::AutomaticBackup, None);
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder_root = root.clone();
        let holder = thread::spawn(move || {
            let guard = crate::repository_lock::acquire_write_gate(
                &holder_root,
                WritePriority::Interactive,
                WaitPolicy::Try,
                &CancelToken::standalone(),
            )
            .unwrap();
            acquired_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
            drop(guard);
        });
        acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("闸门持有者未启动");
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            release_tx.send(()).unwrap();
        });

        coordinator
            .execute(OperationDomain::Library, |context| {
                context.run_write(|_| Ok::<_, CommandError>(()))
            })
            .expect("让位取消不得把排队中的交互写判成已取消");
        assert!(
            background.cancel_token().is_cancelled(),
            "竞争时应请求过后台让位"
        );

        holder.join().unwrap();
        releaser.join().unwrap();
        background.complete();
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
