//! 进程级运行状态：设置快照、仓库租约、仓库校验缓存与退出/诊断标志。
//!
//! `AppState` 只持有进程内状态与锁；它不负责设置文件格式（`model`）或写盘顺序
//! （`service`），也不实现窗口与命令。

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc, Mutex, RwLock,
    },
};

use serde::Serialize;

use crate::app::diagnostics::{self, DiagnosticsState};

use super::model::AppSettings;
use super::service::SettingsService;

/// 诊断阈值：仓库锁等待/持有超过该时长即记录一条告警。正常操作远低于此值，
/// 因此平时零噪声；一旦出现说明存在争用或某次操作异常变慢。
const LOCK_WAIT_WARN_MS: u128 = 300;
const LOCK_HOLD_WARN_MS: u128 = 1_000;
/// 只读操作持有共享租约的告警阈值；纹理解码等本身耗时，阈值放宽。
const READ_HOLD_WARN_MS: u128 = 1_500;

#[derive(Clone)]
pub(crate) struct AppState {
    settings: SettingsService,
    /// 进程角色（`gui` / `headless`），写入 owner lock 元数据用于诊断。
    process_role: &'static str,
    log_directory: PathBuf,
    warning: Arc<RwLock<Option<String>>>,
    validated_repository: Arc<Mutex<Option<PathBuf>>>,
    /// 串行化仓库变更。恢复、精简这类物化任务会持有它数分钟，因此只读命令
    /// 不得获取它。
    repository_operation: Arc<Mutex<()>>,
    /// 所有正在使用仓库的命令共享它，只有仓库路径即将更换时才取写锁。因此长
    /// 恢复或备份运行期间读取照常进行，而切换仓库会等所有在途工作结束再替换
    /// 路径。
    repository_lease: Arc<RwLock<()>>,
    exit_requested: Arc<AtomicBool>,
    /// 关闭/托盘退出的路径开始 webview 关闭握手时置位，重复的关闭请求因此不会
    /// 重启握手。
    shutdown_handshake_started: Arc<AtomicBool>,
    /// webview 确认命令在执行不可逆退出序列前置位，兜底强退计时器据此判断。
    shutdown_confirmed: Arc<AtomicBool>,
    /// 诊断模式开关。决定生效日志等级，以及前端桥接是否写入 info 级事件。
    /// 只存活于当前进程，不写入设置文件，因此正常使用不受影响。
    diagnostics: Arc<DiagnosticsState>,
    /// 最近一次用户长操作开始或结束的时刻。写入命令与内容级只读长任务都会刷新
    /// 它，调度器据此要求后台任务在用户停手后静默一段时间才启动；`0` 表示本
    /// 进程还没有用户长操作。
    last_user_activity_ms: Arc<AtomicI64>,
}

impl AppState {
    pub(crate) fn new(
        settings: AppSettings,
        settings_path: PathBuf,
        log_directory: PathBuf,
        warning: Option<String>,
    ) -> Self {
        Self::with_role(settings, settings_path, log_directory, warning, "gui")
    }

    fn with_role(
        settings: AppSettings,
        settings_path: PathBuf,
        log_directory: PathBuf,
        warning: Option<String>,
        process_role: &'static str,
    ) -> Self {
        // 引用清单是进程级注册：GUI 与无头共用这一处装配点，重复调用只保留
        // 第一次注册的实现列表。
        crate::app::contracts::install_default_reference_inventories();
        Self {
            settings: SettingsService::new(settings, settings_path),
            process_role,
            log_directory,
            warning: Arc::new(RwLock::new(warning)),
            validated_repository: Arc::new(Mutex::new(None)),
            repository_operation: Arc::new(Mutex::new(())),
            repository_lease: Arc::new(RwLock::new(())),
            exit_requested: Arc::new(AtomicBool::new(false)),
            shutdown_handshake_started: Arc::new(AtomicBool::new(false)),
            shutdown_confirmed: Arc::new(AtomicBool::new(false)),
            diagnostics: Arc::new(DiagnosticsState::new(diagnostics::default_enabled())),
            last_user_activity_ms: Arc::new(AtomicI64::new(0)),
        }
    }

    /// 刷新用户长操作的活动时间。命令开始与结束都要调用：开始让调度器知道
    /// 用户正在操作，结束让冷却从最后一刻重新计时。
    pub(crate) fn note_user_activity(&self) {
        if let Ok(now) = crate::storage::now_ms() {
            self.last_user_activity_ms.store(now, Ordering::SeqCst);
        }
    }

    pub(crate) fn last_user_activity_ms(&self) -> Option<i64> {
        match self.last_user_activity_ms.load(Ordering::SeqCst) {
            0 => None,
            value => Some(value),
        }
    }

    /// 无头入口（`feature = "headless"`）使用的构造：仓库路径由命令行显式给定，
    /// 不读取也不写回任何设置文件。设置与日志目录只用于填满字段，无头进程不会
    /// 调用依赖它们的命令。`chunk_format_v2` 由无头的 `--chunk-format` 给出，
    /// 缺省 `true`（与设置默认值一致），只影响本进程新写入的 snapshot/delta。
    #[cfg(feature = "headless")]
    pub(crate) fn for_headless_repository(
        repository: &Path,
        state_directory: &Path,
        chunk_format_v2: bool,
    ) -> Self {
        let mut settings = AppSettings::default();
        settings.repository_path = repository.to_string_lossy().into_owned();
        settings.chunk_file_format_v2 = chunk_format_v2;
        Self::with_role(
            settings,
            state_directory.join("settings.json"),
            state_directory.join("logs"),
            None,
            "headless",
        )
    }

    pub(crate) fn repository_path(&self) -> Result<Option<PathBuf>, String> {
        self.settings.read_with(|settings| {
            let value = settings.repository_path.trim();
            (!value.is_empty()).then(|| PathBuf::from(value))
        })
    }

    pub(crate) fn ready_repository_path(&self) -> Result<PathBuf, String> {
        let started = std::time::Instant::now();
        let root = self.repository_path()?.ok_or("尚未配置作品仓库")?;
        let mut validated = self
            .validated_repository
            .lock()
            .map_err(|_| "仓库校验状态已损坏")?;
        if validated.as_deref() == Some(root.as_path()) {
            if let Err(error) = crate::library::check_existing(&root) {
                *validated = None;
                return Err(error);
            }
            return Ok(root);
        }

        *validated = None;
        if let Err(error) = crate::library::open_existing(&root) {
            log::warn!(
                "repository validation failed: path={}, elapsed_ms={}, error={error}",
                root.display(),
                started.elapsed().as_millis()
            );
            return Err(error);
        }
        *validated = Some(root.clone());
        log::info!(
            "repository validated: path={}, elapsed_ms={}",
            root.display(),
            started.elapsed().as_millis()
        );
        Ok(root)
    }

    pub(crate) fn with_ready_repository<T>(
        &self,
        operation: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_ready_repository_typed(operation)
    }

    /// [`Self::with_ready_repository`] 的 typed 版本：锁与仓库校验失败经 `E: From<String>`
    /// 转成调用方的错误类型，业务闭包保留自己的 typed error。`OperationCoordinator` 用它
    /// 维持固定锁序（运行分类 → 仓库 lease → 仓库操作锁）。
    pub(crate) fn with_ready_repository_typed<T, E: From<String>>(
        &self,
        operation: impl FnOnce(&Path) -> Result<T, E>,
    ) -> Result<T, E> {
        let wait_started = std::time::Instant::now();
        let _lease = self
            .repository_lease
            .read()
            .map_err(|_| E::from("仓库租约已损坏".to_owned()))?;
        let _operation = self
            .repository_operation
            .lock()
            .map_err(|_| E::from("仓库操作锁已损坏".to_owned()))?;
        // 只在真的等待时才记录：用于判断前端卡死是否伴随仓库锁争用。
        let waited_ms = wait_started.elapsed().as_millis();
        if waited_ms > LOCK_WAIT_WARN_MS {
            log::warn!("[slow] repository mutation lock waited {waited_ms} ms");
        }
        let root = self.ready_repository_path().map_err(E::from)?;
        let held_started = std::time::Instant::now();
        let result = operation(&root);
        let held_ms = held_started.elapsed().as_millis();
        if held_ms > LOCK_HOLD_WARN_MS {
            log::warn!("[slow] repository mutation held the lock for {held_ms} ms");
        }
        result
    }

    /// 运行一次只读仓库操作。
    ///
    /// 读取共享仓库租约而不是变更锁，因此长备份、恢复或精简运行期间，树刷新、
    /// 历史读取与设置快照仍能作答。写事务提交时 SQLite 的 WAL 保证每次读取
    /// 看到一致状态。
    pub(crate) fn with_repository_read<T>(
        &self,
        operation: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_repository_read_labeled("", operation)
    }

    /// 与 [`Self::with_repository_read`] 相同，但超时告警会带上 `label`。
    ///
    /// 用途：把「读操作自身耗时过长」归因到具体命令。素材板卡死期间曾观察到
    /// `repository read took 3000+ ms`，但无从判断是纹理读取、树刷新还是别处；
    /// 调用点传入 `read_pin_board_texture board=.. image=..` 这类标签即可定位。
    pub(crate) fn with_repository_read_labeled<T>(
        &self,
        label: &str,
        operation: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_repository_read_labeled_typed(label, operation)
    }

    /// [`Self::with_repository_read_labeled`] 的 typed 版本。
    pub(crate) fn with_repository_read_labeled_typed<T, E: From<String>>(
        &self,
        label: &str,
        operation: impl FnOnce(&Path) -> Result<T, E>,
    ) -> Result<T, E> {
        // 空标签不打印括号，避免日志里出现无意义的 `()`。
        let suffix = if label.is_empty() {
            String::new()
        } else {
            format!(" [{label}]")
        };
        let wait_started = std::time::Instant::now();
        let _lease = self
            .repository_lease
            .read()
            .map_err(|_| E::from("仓库租约已损坏".to_owned()))?;
        let waited_ms = wait_started.elapsed().as_millis();
        if waited_ms > LOCK_WAIT_WARN_MS {
            log::warn!("[slow] repository read lease waited {waited_ms} ms{suffix}");
        }
        let root = self.ready_repository_path().map_err(E::from)?;
        let ran_started = std::time::Instant::now();
        let result = operation(&root);
        let ran_ms = ran_started.elapsed().as_millis();
        if ran_ms > READ_HOLD_WARN_MS {
            log::warn!("[slow] repository read took {ran_ms} ms{suffix}");
        }
        result
    }

    #[cfg(test)]
    fn with_repository_switch<T>(
        &self,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_repository_switch_typed(operation)
    }

    /// 设置保存专用的 typed 版本：在同一临界区里替换仓库路径并写设置；协调器先取得
    /// 运行分类锁，再进入这里取写租约。非 typed 的 `with_repository_switch` 只供测试
    /// 使用（`#[cfg(test)]`）。
    pub(crate) fn with_repository_switch_typed<T, E: From<String>>(
        &self,
        operation: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let _lease = self
            .repository_lease
            .write()
            .map_err(|_| E::from("仓库租约已损坏".to_owned()))?;
        let _operation = self
            .repository_operation
            .lock()
            .map_err(|_| E::from("仓库操作锁已损坏".to_owned()))?;
        operation()
    }

    pub(crate) fn settings(&self) -> &SettingsService {
        &self.settings
    }

    pub(crate) fn process_role(&self) -> &'static str {
        self.process_role
    }

    pub(crate) fn settings_path(&self) -> &Path {
        self.settings.path()
    }

    pub(crate) fn clear_warning(&self) -> Result<(), String> {
        *self.warning.write().map_err(|_| "设置警告状态已损坏")? = None;
        Ok(())
    }

    pub(crate) fn set_validated_repository(&self, root: Option<PathBuf>) -> Result<(), String> {
        *self
            .validated_repository
            .lock()
            .map_err(|_| "仓库校验状态已损坏")? = root;
        Ok(())
    }

    pub(crate) fn close_to_tray(&self) -> bool {
        self.settings
            .read_with(|settings| settings.close_to_tray)
            .unwrap_or(true)
    }

    pub(crate) fn automatic_backups_paused(&self) -> bool {
        self.settings
            .read_with(|settings| settings.pause_automatic_backups)
            .unwrap_or(false)
    }

    /// 空闲完整性检查是否开启。读取失败按关闭处理：后台检查是可选能力，
    /// 设置状态异常时不应擅自开始扫描仓库。
    pub(crate) fn idle_integrity_check_enabled(&self) -> bool {
        self.settings
            .read_with(|settings| settings.idle_integrity_check_enabled)
            .unwrap_or(false)
    }

    /// 自动备份默认检查方式是否为快速。仅在设置显式选择 full 时返回 false；
    /// 设置缺失或状态损坏时保持默认的快速检查。
    pub(crate) fn automatic_backup_quick_default(&self) -> bool {
        self.settings
            .read_with(|settings| settings.automatic_backup_check_mode.as_str() != "full")
            .unwrap_or(true)
    }

    /// 新写入的 ChunkFile 版本。默认 v2；设置损坏或缺失时保持 v2。
    pub(crate) fn chunk_write_options(&self) -> crate::backup::chunk_file::ChunkWriteOptions {
        let enabled = self
            .settings
            .read_with(|settings| settings.chunk_file_format_v2)
            .unwrap_or(true);
        if enabled {
            crate::backup::chunk_file::ChunkWriteOptions::V2
        } else {
            crate::backup::chunk_file::ChunkWriteOptions::V1
        }
    }

    /// 素材板原生纹理解码结果缓存等级（low/medium/high）。
    pub(crate) fn pin_board_texture_cache_level(&self) -> String {
        self.settings
            .read_with(|settings| settings.pin_board.texture_cache_level.clone())
            .unwrap_or_else(|_| "medium".into())
    }

    /// 诊断模式状态；等级切换与前端桥接的写入门控都读这里。
    pub(crate) fn diagnostics(&self) -> &DiagnosticsState {
        &self.diagnostics
    }

    /// 应用日志目录，供诊断状态展示与「打开日志目录」命令使用。
    pub(crate) fn log_directory(&self) -> &Path {
        &self.log_directory
    }

    pub(crate) fn request_exit(&self) {
        self.exit_requested.store(true, Ordering::SeqCst);
    }

    pub(crate) fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::SeqCst)
    }

    /// 标记关闭握手已开始；若它已在运行则返回 false，让重复的关闭请求让路。
    pub(crate) fn begin_shutdown_handshake(&self) -> bool {
        !self.shutdown_handshake_started.swap(true, Ordering::SeqCst)
    }

    /// 标记 webview 确认已被消费；若另一个确认已启动退出序列则返回 false。
    pub(crate) fn mark_shutdown_confirmed(&self) -> bool {
        !self.shutdown_confirmed.swap(true, Ordering::SeqCst)
    }

    pub(crate) fn shutdown_confirmed(&self) -> bool {
        self.shutdown_confirmed.load(Ordering::SeqCst)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSnapshot {
    pub(crate) settings: AppSettings,
    pub(crate) settings_path: String,
    pub(crate) log_directory: String,
    pub(crate) warning: Option<String>,
    pub(crate) automatic_backup_file_count: Option<usize>,
    /// 待校验的历史节点数（与空闲检查的派生集合同一口径）。
    pub(crate) idle_integrity_pending_count: Option<usize>,
}

pub(crate) fn snapshot(state: &AppState) -> Result<SettingsSnapshot, String> {
    // 两个计数共用一次仓库就绪校验：`ready_repository_path` 每次都做一遍轻量
    // format/version 检查，同一份快照里没必要跑两遍。
    let repository = state.ready_repository_path().ok();
    let automatic_backup_file_count = repository
        .as_deref()
        .and_then(|root| crate::history::count_scheduled_files(root).ok());
    let idle_integrity_pending_count = repository
        .as_deref()
        .and_then(|root| crate::history::count_pending_verify_nodes(root).ok());
    Ok(SettingsSnapshot {
        settings: state.settings.read().map_err(|_| "设置状态已损坏")?,
        settings_path: state.settings_path().to_string_lossy().into_owned(),
        log_directory: state.log_directory.to_string_lossy().into_owned(),
        warning: state
            .warning
            .read()
            .map_err(|_| "设置警告状态已损坏")?
            .clone(),
        automatic_backup_file_count,
        idle_integrity_pending_count,
    })
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread, time::Duration};

    use super::*;

    #[test]
    fn repository_readiness_caches_integrity_check_and_rechecks_version() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let mut settings = AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();
        let state = AppState::new(
            settings,
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        crate::library::take_integrity_check_count();
        // 记录当前版本，用于后面「把版本改坏再改回来」。写死 v5 会在下一次 schema
        // 升级后变成一次真实迁移（表已在 v6 形状），这条缓存测试就不再是它要测的东西。
        let current_version: String = crate::storage::open(&root)
            .unwrap()
            .query_row(
                "SELECT value FROM repository_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(state.ready_repository_path().unwrap(), root);
        assert_eq!(state.ready_repository_path().unwrap(), root);
        assert_eq!(crate::library::take_integrity_check_count(), 1);

        crate::storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE repository_meta SET value = '99' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        let error = state.ready_repository_path().unwrap_err();
        assert!(error.contains("更新版本"), "{error}");
        assert_eq!(crate::library::take_integrity_check_count(), 0);

        crate::storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE repository_meta SET value = ?1 WHERE key = 'schema_version'",
                [&current_version],
            )
            .unwrap();
        assert_eq!(state.ready_repository_path().unwrap(), root);
        assert_eq!(crate::library::take_integrity_check_count(), 1);

        crate::storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE repository_meta SET value = 'other' WHERE key = 'format'",
                [],
            )
            .unwrap();
        let error = state.ready_repository_path().unwrap_err();
        assert!(error.contains("不是 Lilith Artworks 仓库"), "{error}");
        assert_eq!(crate::library::take_integrity_check_count(), 0);
    }

    #[test]
    fn repository_switch_waits_for_an_active_repository_lease() {
        let directory = tempfile::tempdir().unwrap();
        let first_root = directory.path().join("first");
        let second_root = directory.path().join("second");
        crate::library::initialize(&first_root).unwrap();
        crate::library::initialize(&second_root).unwrap();
        let mut settings = AppSettings::default();
        settings.repository_path = first_root.to_string_lossy().into_owned();
        let state = AppState::new(
            settings,
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let active = state.clone();
        let active_thread = thread::spawn(move || {
            active
                .with_ready_repository(|_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        });
        entered_rx.recv().unwrap();

        let (switched_tx, switched_rx) = mpsc::channel();
        let switching = state.clone();
        let switching_thread = thread::spawn(move || {
            switching
                .with_repository_switch(|| {
                    switching
                        .settings
                        .update(None, |settings| {
                            settings.repository_path = second_root.to_string_lossy().into_owned();
                            Ok(())
                        })
                        .unwrap();
                    switched_tx.send(()).unwrap();
                    Ok(())
                })
                .unwrap();
        });

        assert!(matches!(
            switched_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release_tx.send(()).unwrap();
        switched_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        active_thread.join().unwrap();
        switching_thread.join().unwrap();
    }

    /// 长写任务持仓库操作锁期间，只读命令必须仍能作答。
    ///
    /// 这是产品明文的并发承诺（`with_ready_repository` 的排他锁「会被 restore/compaction
    /// 持有数分钟」，而只读命令走共享读租约），此前只有「切换仓库会等活跃租约」被验证过，
    /// 「长写期间读还能不能返回」没有断言。回归会表现为只读命令被排他锁挡住——即素材板
    /// 在备份/恢复期间卡死。这里用线程与超时把它钉死：若读路径错误地取了排他锁，读者不会
    /// 在超时内返回，测试失败而不是无限挂起。
    #[test]
    fn repository_read_answers_while_a_long_write_holds_the_operation_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let mut settings = AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();
        let state = AppState::new(
            settings,
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );

        let (write_entered_tx, write_entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = state.clone();
        let writer_thread = thread::spawn(move || {
            writer
                .with_ready_repository(|_| {
                    // 进入操作锁后停住，模拟一次耗时数分钟的长任务仍持有排他锁。
                    write_entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        });
        write_entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("写线程未进入仓库操作锁");

        // 写操作此刻持有排他锁：只读命令走共享读租约，必须立刻返回。
        let (read_tx, read_rx) = mpsc::channel();
        let reader = state.clone();
        let reader_thread = thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = reader.with_repository_read(|root| Ok(root.is_dir()));
            read_tx.send((result, started.elapsed())).unwrap();
        });

        let (result, waited) = read_rx.recv_timeout(Duration::from_secs(5)).expect(
            "只读命令在长写期间未能返回——很可能错误地取了排他仓库操作锁（会让界面在备份/恢复时卡死）",
        );
        assert!(
            result.expect("长写期间只读命令必须成功"),
            "只读命令在长写期间应能读到仓库目录"
        );
        assert!(
            waited < Duration::from_secs(2),
            "只读命令在长写期间等待了 {waited:?}，说明它被排他锁挡住了"
        );

        release_tx.send(()).unwrap();
        writer_thread.join().unwrap();
        reader_thread.join().unwrap();
    }
}
