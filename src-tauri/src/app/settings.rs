use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, State};
use tempfile::NamedTempFile;

use super::diagnostics::{self, DiagnosticsState};
use super::{
    command_runtime::{CommandError, OperationDomain},
    usecases, OperationCoordinator,
};
use crate::backup::BackupState;
use crate::platform::{
    path::ensure_not_device_path,
    shell::{open_directory, open_existing_directory, reveal_in_file_manager},
};
use crate::{authenticity::pipeline, cleanup, history, library};

pub(crate) const CURRENT_SETTINGS_VERSION: u32 = 2;

/// 主窗口允许的最小物理尺寸，与 `tauri.conf.json` 的 `minWidth` / `minHeight` 一致。
/// 加载设置时用它夹紧越界值，捕获窗口几何时用它判断一次捕获是否可信。
const MIN_WINDOW_WIDTH: u32 = 760;
const MIN_WINDOW_HEIGHT: u32 = 560;
/// 主窗口允许的最大物理尺寸；超过它几乎只可能来自异常状态或异常的显示器几何。
const MAX_WINDOW_WIDTH: u32 = 16_384;
const MAX_WINDOW_HEIGHT: u32 = 16_384;
/// 恢复窗口位置的最低门槛：窗口矩形同某块显示器的重叠不足这个范围时，视为落在
/// 已经不存在的显示器上，改为居中，避免窗口开到屏幕外让用户以为程序没启动。
const MIN_VISIBLE_WINDOW_WIDTH: i32 = 160;
const MIN_VISIBLE_WINDOW_HEIGHT: i32 = 80;

/// 诊断阈值：仓库锁等待/持有超过该时长即记录一条告警。正常操作远低于此值，
/// 因此平时零噪声；一旦出现说明存在争用或某次操作异常变慢。
const LOCK_WAIT_WARN_MS: u128 = 300;
const LOCK_HOLD_WARN_MS: u128 = 1_000;
/// 只读操作持有共享租约的告警阈值；纹理解码等本身耗时，阈值放宽。
const READ_HOLD_WARN_MS: u128 = 1_500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AppSettings {
    pub(crate) version: u32,
    /// 设置文件自己的单调计数：每次成功写入 +1。设置页保存用它做 CAS；它与
    /// schema、ChunkFile 格式和发布版本号无关。
    #[serde(default)]
    pub(crate) revision: u64,
    pub(crate) repository_path: String,
    theme: String,
    close_to_tray: bool,
    pub(crate) pause_automatic_backups: bool,
    /// 空闲完整性检查开关。默认关闭；旧设置文件缺少该字段时同样视为关闭，
    /// 因此不需要提升设置版本。
    pub(crate) idle_integrity_check_enabled: bool,
    automatic_backup_check_mode: String,
    /// ChunkFile v2 写入开关。默认开启：只影响此后新写出的 snapshot/delta，
    /// 旧 v1 节点继续由 v1 reader 读取；关闭后新写入回到 v1。
    chunk_file_format_v2: bool,
    window: WindowSettings,
    content: ContentSettings,
    pin_board: PinBoardSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: CURRENT_SETTINGS_VERSION,
            revision: 0,
            repository_path: String::new(),
            theme: "system".into(),
            close_to_tray: true,
            pause_automatic_backups: false,
            idle_integrity_check_enabled: false,
            automatic_backup_check_mode: "quick".into(),
            chunk_file_format_v2: true,
            window: WindowSettings::default(),
            content: ContentSettings::default(),
            pin_board: PinBoardSettings::default(),
        }
    }
}

/// 素材板显示与性能设置。缓存等级与间距由前端模块和原生纹理缓存预算共同消费。
/// 新增字段依赖容器级 `#[serde(default)]` 兼容旧设置文件，无需提升设置版本。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PinBoardSettings {
    texture_cache_level: String,
    arrangement_gap_px: f64,
    /// 编辑停顿后的防抖自动保存（前端行为），默认关闭。
    autosave: bool,
    /// 退出应用前结算并保存素材板（关闭握手），默认开启。
    save_on_exit: bool,
    lock_shortcut: String,
    fullscreen_shortcut: String,
}

impl Default for PinBoardSettings {
    fn default() -> Self {
        Self {
            texture_cache_level: "medium".into(),
            arrangement_gap_px: 10.0,
            autosave: false,
            save_on_exit: true,
            lock_shortcut: "CommandOrControl+R".into(),
            fullscreen_shortcut: "F11".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct WindowSettings {
    x: Option<i32>,
    y: Option<i32>,
    width: u32,
    height: u32,
    maximized: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: 1320,
            height: 840,
            maximized: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ContentSettings {
    density: String,
    default_panel: String,
}

impl Default for ContentSettings {
    fn default() -> Self {
        Self {
            density: "comfortable".into(),
            default_panel: "overview".into(),
        }
    }
}

/// 设置的唯一写入口。
///
/// 设置页保存走 [`Self::update`] 并携带期望 revision；托盘、窗口几何等应用内部
/// 写入也经同一入口，只是在当前快照上打补丁（无 CAS）。写入顺序固定为：取写锁 →
/// 复核 revision → 原子替换 JSON → 更新内存快照；任何一步失败都不会留下内存与
/// 文件不一致的状态。
#[derive(Clone)]
pub(crate) struct SettingsService {
    inner: Arc<SettingsServiceInner>,
}

struct SettingsServiceInner {
    current: RwLock<AppSettings>,
    path: PathBuf,
    write: Mutex<()>,
}

impl SettingsService {
    fn new(settings: AppSettings, path: PathBuf) -> Self {
        Self {
            inner: Arc::new(SettingsServiceInner {
                current: RwLock::new(settings),
                path,
                write: Mutex::new(()),
            }),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.inner.path
    }

    fn read(&self) -> Result<AppSettings, String> {
        self.inner
            .current
            .read()
            .map(|settings| settings.clone())
            .map_err(|_| "设置状态已损坏".to_owned())
    }

    fn read_with<T>(&self, reader: impl FnOnce(&AppSettings) -> T) -> Result<T, String> {
        self.inner
            .current
            .read()
            .map(|settings| reader(&settings))
            .map_err(|_| "设置状态已损坏".to_owned())
    }

    pub(crate) fn update(
        &self,
        expected_revision: Option<u64>,
        patch: impl FnOnce(&mut AppSettings) -> Result<(), CommandError>,
    ) -> Result<AppSettings, CommandError> {
        let _writing = self
            .inner
            .write
            .lock()
            .map_err(|_| CommandError::internal("设置写入锁已损坏"))?;
        let current = self
            .inner
            .current
            .read()
            .map(|settings| settings.clone())
            .map_err(|_| CommandError::internal("设置状态已损坏"))?;
        if expected_revision.is_some_and(|expected| expected != current.revision) {
            return Err(CommandError::settings_conflict());
        }
        let mut next = current.clone();
        patch(&mut next)?;
        next.revision = current.revision.saturating_add(1);
        write_json_atomic(&self.inner.path, &next).map_err(CommandError::from)?;
        *self
            .inner
            .current
            .write()
            .map_err(|_| CommandError::internal("设置状态已损坏"))? = next.clone();
        Ok(next)
    }
}

#[derive(Clone)]
pub(crate) struct AppState {
    settings: SettingsService,
    /// 进程角色（`gui` / `headless`），写入 owner lock 元数据用于诊断。
    process_role: &'static str,
    log_directory: PathBuf,
    warning: Arc<RwLock<Option<String>>>,
    validated_repository: Arc<Mutex<Option<PathBuf>>>,
    /// Serializes repository mutations. Materialization work such as restore or
    /// compaction holds this lock for minutes, so read-only commands must not
    /// take it.
    repository_operation: Arc<Mutex<()>>,
    /// Shared by every command that is using the repository, and taken
    /// exclusively only when the repository path is about to change. Reads
    /// therefore keep working while a long restore or backup runs, and a switch
    /// still waits for all in-flight work before replacing the path.
    repository_lease: Arc<RwLock<()>>,
    exit_requested: Arc<AtomicBool>,
    /// Set once when the close/tray-quit path starts the webview shutdown
    /// handshake, so repeated close requests do not restart it.
    shutdown_handshake_started: Arc<AtomicBool>,
    /// Set once by the webview confirmation command before running the
    /// irreversible shutdown sequence; the fallback force-exit timer checks it.
    shutdown_confirmed: Arc<AtomicBool>,
    /// 诊断模式开关。决定生效日志等级，以及前端桥接是否写入 info 级事件。
    /// 只存活于当前进程，不写入设置文件，因此正常使用不受影响。
    diagnostics: Arc<DiagnosticsState>,
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
        super::contracts::install_default_reference_inventories();
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
            if let Err(error) = library::check_existing(&root) {
                *validated = None;
                return Err(error);
            }
            return Ok(root);
        }

        *validated = None;
        if let Err(error) = library::open_existing(&root) {
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

    /// Runs a read-only repository operation.
    ///
    /// Reads share the repository lease instead of the mutation lock, so a tree
    /// refresh, history read or settings snapshot still answers while a long
    /// backup, restore or compaction is running. SQLite's WAL keeps each read
    /// consistent while a write transaction commits.
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

    #[allow(dead_code)]
    fn with_repository_switch<T>(
        &self,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_repository_switch_typed(operation)
    }

    /// [`Self::with_repository_switch`] 的 typed 版本。设置保存需要在同一临界区里
    /// 替换仓库路径并写设置；协调器先取得运行分类锁，再进入这里取写租约。
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

    /// Marks the shutdown handshake as started; returns false when it was
    /// already running, so duplicate close requests keep out of the way.
    pub(crate) fn begin_shutdown_handshake(&self) -> bool {
        !self.shutdown_handshake_started.swap(true, Ordering::SeqCst)
    }

    /// Marks the webview confirmation as consumed; returns false when another
    /// confirmation already started the exit sequence.
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

/// 旧版本设置迁移。
///
/// v1 曾把素材板“锁定画板”的默认键设为 `CommandOrControl+Shift+K`；该默认值已改回
/// Client 的 `CommandOrControl+R`（整页刷新由应用层拦截），因此把仍是旧默认值的
/// 持久化设置一并升级，用户自定义过的其他键位保持不变。
fn migrate_settings(settings: &mut AppSettings) {
    if settings.version < 2 && settings.pin_board.lock_shortcut == "CommandOrControl+Shift+K" {
        settings.pin_board.lock_shortcut = "CommandOrControl+R".into();
    }
}

/// 快捷键允许留空（表示未设置）；非空时按键组合必须由非空段组成，长度受限。
fn shortcut_is_well_formed(shortcut: &str) -> bool {
    let shortcut = shortcut.trim();
    shortcut.is_empty()
        || (shortcut.chars().count() <= 64
            && !shortcut.split('+').any(|part| part.trim().is_empty()))
}

/// 加载落盘设置时的宽松修正：只把越界的个别字段改回可用值，绝不整份丢弃。
///
/// 与 [`validate_settings`] 的分工不同——后者服务于保存入口，用户输入必须明确拒绝；
/// 加载时一个坏字段（典型来源是窗口最小化退出时被写进来的任务栏图标尺寸）不应该把
/// 用户的仓库路径和全部偏好一起清空。返回是否做过修正，供调用点决定是否留下告警日志。
fn sanitize_settings(settings: &mut AppSettings) -> bool {
    let fallback = AppSettings::default();
    let mut repaired = false;

    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        settings.theme = fallback.theme;
        repaired = true;
    }
    if !matches!(
        settings.automatic_backup_check_mode.as_str(),
        "quick" | "full"
    ) {
        settings.automatic_backup_check_mode = fallback.automatic_backup_check_mode;
        repaired = true;
    }
    if !matches!(settings.content.density.as_str(), "comfortable" | "compact") {
        settings.content.density = fallback.content.density;
        repaired = true;
    }
    if !matches!(
        settings.content.default_panel.as_str(),
        "overview" | "history" | "authenticity"
    ) {
        settings.content.default_panel = fallback.content.default_panel;
        repaired = true;
    }
    if !matches!(
        settings.pin_board.texture_cache_level.as_str(),
        "low" | "medium" | "high"
    ) {
        settings.pin_board.texture_cache_level = fallback.pin_board.texture_cache_level;
        repaired = true;
    }
    if !(1.0..=200.0).contains(&settings.pin_board.arrangement_gap_px) {
        settings.pin_board.arrangement_gap_px = fallback.pin_board.arrangement_gap_px;
        repaired = true;
    }
    if !shortcut_is_well_formed(&settings.pin_board.lock_shortcut) {
        settings.pin_board.lock_shortcut = fallback.pin_board.lock_shortcut.clone();
        repaired = true;
    }
    if !shortcut_is_well_formed(&settings.pin_board.fullscreen_shortcut) {
        settings.pin_board.fullscreen_shortcut = fallback.pin_board.fullscreen_shortcut.clone();
        repaired = true;
    }

    // 窗口尺寸只夹紧到合法范围，而不是判整份设置无效：越界尺寸最常见的来源是
    // 最小化时捕获到的任务栏图标矩形（约 237×39），夹回最小尺寸即可让窗口继续可见。
    let width = settings
        .window
        .width
        .clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_WIDTH);
    if width != settings.window.width {
        settings.window.width = width;
        repaired = true;
    }
    let height = settings
        .window
        .height
        .clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_HEIGHT);
    if height != settings.window.height {
        settings.window.height = height;
        repaired = true;
    }

    repaired
}

pub(crate) fn load_settings(path: &Path) -> (AppSettings, Option<String>) {
    if !path.exists() {
        let settings = AppSettings::default();
        let warning = write_json_atomic(path, &settings)
            .err()
            .map(|error| format!("无法创建默认设置：{error}"));
        return (settings, warning);
    }

    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<AppSettings>(&content) {
            Ok(mut settings) if settings.version == CURRENT_SETTINGS_VERSION => {
                // 越界字段就地修正而不是整份回落默认：曾经把仓库路径连同全部偏好
                // 一起清空。修正只发生在内存里，一致性交回既有写入路径（保存设置或
                // 退出时捕获窗口），启动阶段不额外写盘。
                if sanitize_settings(&mut settings) {
                    log::warn!(
                        "settings.json had out-of-range fields; repaired them in place without dropping the rest"
                    );
                }
                (settings, None)
            }
            Ok(settings) if settings.version > CURRENT_SETTINGS_VERSION => (
                settings,
                Some("settings.json 来自更高版本，本次运行不会覆盖它".into()),
            ),
            Ok(mut settings) => {
                migrate_settings(&mut settings);
                settings.version = CURRENT_SETTINGS_VERSION;
                sanitize_settings(&mut settings);
                let warning = write_json_atomic(path, &settings)
                    .err()
                    .map(|error| format!("设置迁移后无法写回：{error}"));
                (settings, warning)
            }
            Err(error) => (
                AppSettings::default(),
                Some(format!("settings.json 格式无效，已临时使用默认值：{error}")),
            ),
        },
        Err(error) => (
            AppSettings::default(),
            Some(format!("无法读取 settings.json，已临时使用默认值：{error}")),
        ),
    }
}

#[tauri::command]
pub(crate) async fn get_app_settings(
    coordinator: State<'_, OperationCoordinator>,
) -> Result<SettingsSnapshot, String> {
    let coordinator = coordinator.inner().clone();
    coordinator
        .spawn(OperationDomain::Settings, |context| {
            usecases::settings::snapshot_for_command(context)
        })
        .await
        .map_err(String::from)
}

#[tauri::command]
pub(crate) fn open_log_directory(state: State<'_, AppState>) -> Result<(), String> {
    open_directory(&state.log_directory, "日志")
}

#[tauri::command]
pub(crate) fn open_settings_directory(state: State<'_, AppState>) -> Result<(), String> {
    let directory = state.settings_path().parent().ok_or("设置文件路径无效")?;
    open_directory(directory, "设置")
}

/// 在系统文件管理器中打开给定文件所在的文件夹（Windows 上会选中该文件）。
#[tauri::command]
pub(crate) fn reveal_path_in_folder(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("文件路径为空".into());
    }
    // 用户在场的手动操作：网络位置照常打开（NAS 是合法用法），但设备命名空间
    // 一律拒绝——那是设备对象而不是文件，交给资源管理器打开没有意义。
    ensure_not_device_path(trimmed, "文件路径")?;
    let path = Path::new(trimmed);
    if !path.is_file() {
        return Err("文件不存在或已被移动".into());
    }
    reveal_in_file_manager(path)
}

#[tauri::command]
pub(crate) fn open_legal_directory(app: AppHandle) -> Result<(), String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| format!("无法定位应用资源目录：{error}"))?;
    let candidates = [
        resource_dir.join("licenses"),
        resource_dir.join("resources").join("licenses"),
    ];
    let directory = candidates
        .iter()
        .find(|path| path.join("THIRD_PARTY_LICENSES.html").is_file())
        .ok_or("随包法律材料缺失；请重新安装应用")?;
    open_existing_directory(directory, "法律材料")
}

#[tauri::command]
pub(crate) async fn save_app_settings(
    app: AppHandle,
    coordinator: State<'_, OperationCoordinator>,
    settings: AppSettings,
) -> Result<SettingsSnapshot, String> {
    let coordinator = coordinator.inner().clone();
    let next = coordinator
        .spawn(OperationDomain::Settings, move |context| {
            usecases::settings::save(context, settings)
        })
        .await
        .map_err(String::from)?;
    crate::refresh_tray_backup_menu(&app)?;
    Ok(next)
}

pub(crate) fn prepare_repository(
    current_repository: Option<&Path>,
    requested: &str,
) -> Result<Option<PathBuf>, String> {
    if requested.is_empty() {
        return Ok(None);
    }
    let root = Path::new(requested);
    if current_repository == Some(root) {
        library::open_existing(root)?;
    } else {
        library::initialize(root)?;
    }
    sweep_stale_repository_temp(root);
    Ok(Some(root.to_path_buf()))
}

/// 仓库就绪后清扫超期的仓库 `temp/`（认证预览会话目录与 `.tmp*` 暂存残留）。
/// 清扫只碰不被引用的临时条目，失败只记日志，绝不阻断仓库打开。
fn sweep_stale_repository_temp(root: &Path) {
    let report = cleanup::sweep_repository_temp(root, pipeline::preview_session_id());
    if report.reclaimed > 0 || report.failed > 0 {
        log::info!(
            "仓库打开时清扫临时目录：回收 {} 项，失败 {} 项",
            report.reclaimed,
            report.failed
        );
    }
}

pub(crate) fn set_automatic_backups_paused(app: &AppHandle, paused: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    state
        .settings()
        .update(None, |settings| {
            settings.pause_automatic_backups = paused;
            Ok(())
        })
        .map_err(String::from)?;
    let backup = app.state::<BackupState>();
    backup.set_automatic_scheduling(!paused);
    backup.wake_scheduler();
    crate::refresh_tray_backup_menu(app)?;
    Ok(())
}

/// 一块显示器在虚拟桌面坐标里的矩形，独立于窗口系统，便于做纯几何单元测试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MonitorRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// 读取当前所有显示器的矩形；拿不到时返回 `None`，由调用点回退到安全行为。
fn monitor_rects(window: &tauri::WebviewWindow) -> Option<Vec<MonitorRect>> {
    window.available_monitors().ok().map(|monitors| {
        monitors
            .iter()
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                MonitorRect {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }
            })
            .collect()
    })
}

/// 窗口矩形与任意一块显示器是否重叠出足以让用户看到并拖动它的范围。
///
/// 保存窗口时的显示器可能已经断开（外接绘画屏、投影）或分辨率改变；只按保存的坐标
/// 恢复会让窗口开在屏幕外。只蹭到一条边同样按不可见处理。
fn window_overlaps_a_monitor(
    monitors: &[MonitorRect],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> bool {
    // 尺寸已在调用点限制在主窗口允许范围内；这里再做一次饱和转换，
    // 避免任何 u32 -> i32 溢出改变几何判断。
    let width = width.min(i32::MAX as u32) as i32;
    let height = height.min(i32::MAX as u32) as i32;
    let right = x.saturating_add(width);
    let bottom = y.saturating_add(height);
    let required_width = MIN_VISIBLE_WINDOW_WIDTH.min(width).max(1);
    let required_height = MIN_VISIBLE_WINDOW_HEIGHT.min(height).max(1);
    monitors.iter().any(|monitor| {
        let monitor_right = monitor
            .x
            .saturating_add(monitor.width.min(i32::MAX as u32) as i32);
        let monitor_bottom = monitor
            .y
            .saturating_add(monitor.height.min(i32::MAX as u32) as i32);
        let overlap_width = right.min(monitor_right) - x.max(monitor.x);
        let overlap_height = bottom.min(monitor_bottom) - y.max(monitor.y);
        overlap_width >= required_width && overlap_height >= required_height
    })
}

/// 保存的窗口坐标是否仍落在当前某块显示器上。拿不到显示器列表时按不可见处理，
/// 让调用点回退到居中，而不是把窗口恢复到未知位置。
fn saved_position_is_on_a_current_monitor(
    window: &tauri::WebviewWindow,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> bool {
    monitor_rects(window)
        .map(|monitors| window_overlaps_a_monitor(&monitors, x, y, width, height))
        .unwrap_or(false)
}

/// 一次窗口尺寸捕获是否可信：只接受落在主窗口允许范围内的物理尺寸。
///
/// 最小化时 `GetWindowRect` 返回的是任务栏图标矩形（实测约 237×39），最大化时是铺满
/// 屏幕的矩形；两者都不是用户摆放的窗口几何，必须挡在写入设置之前。
fn plausible_window_size(width: u32, height: u32) -> bool {
    (MIN_WINDOW_WIDTH..=MAX_WINDOW_WIDTH).contains(&width)
        && (MIN_WINDOW_HEIGHT..=MAX_WINDOW_HEIGHT).contains(&height)
}

pub(crate) fn restore_window_settings(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let settings = state.settings().read().map_err(|_| "设置状态已损坏")?;
    let window = app.get_webview_window("main").ok_or("找不到主窗口")?;
    // 这里再夹紧一次：来自更高版本的设置文件不会走 `sanitize_settings`，保证无论
    // 来源如何都不会把越界尺寸喂给窗口。
    let width = settings
        .window
        .width
        .clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_WIDTH);
    let height = settings
        .window
        .height
        .clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_HEIGHT);
    window
        .set_size(PhysicalSize::new(width, height))
        .map_err(|error| format!("无法恢复窗口大小：{error}"))?;
    // 只有保存的坐标仍落在当前某块显示器上时才恢复位置：保存时的显示器可能已经
    // 断开，直接恢复会让窗口开在屏幕外，用户会以为程序没有启动。
    let saved_position =
        settings.window.x.zip(settings.window.y).filter(|(x, y)| {
            saved_position_is_on_a_current_monitor(&window, *x, *y, width, height)
        });
    match saved_position {
        Some((x, y)) => window
            .set_position(PhysicalPosition::new(x, y))
            .map_err(|error| format!("无法恢复窗口位置：{error}"))?,
        None => window
            .center()
            .map_err(|error| format!("无法居中主窗口：{error}"))?,
    }
    if settings.window.maximized {
        window
            .maximize()
            .map_err(|error| format!("无法恢复最大化状态：{error}"))?;
    }
    Ok(())
}

pub(crate) fn capture_window_settings(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let window = app.get_webview_window("main").ok_or("找不到主窗口")?;
    let maximized = window.is_maximized().unwrap_or(false);
    // 最小化时窗口矩形是任务栏图标（约 237×39、坐标 -32000），最大化时是铺满屏幕的
    // 矩形：把它们写进设置会让下次启动的尺寸校验失败，进而整份设置回落默认，所以只在
    // 正常窗口状态下捕获几何；最小化/最大化仍会记录 `maximized` 标志。
    let inspectable = !window.is_minimized().unwrap_or(false) && !maximized;
    let size = inspectable
        .then(|| window.outer_size().ok())
        .flatten()
        .filter(|size| plausible_window_size(size.width, size.height));

    // 位置校验需要窗口尺寸：优先用刚捕获的值，拿不到时退回设置里的旧值。
    let (width, height) = match size {
        Some(size) => (size.width, size.height),
        None => state
            .settings()
            .read_with(|settings| (settings.window.width, settings.window.height))
            .map_err(|_| "设置状态已损坏")?,
    };
    let position = inspectable
        .then(|| window.outer_position().ok())
        .flatten()
        .filter(|position| {
            saved_position_is_on_a_current_monitor(&window, position.x, position.y, width, height)
        });

    state
        .settings()
        .update(None, |settings| {
            settings.window.maximized = maximized;
            if let Some(size) = size {
                settings.window.width = size.width;
                settings.window.height = size.height;
            }
            if let Some(position) = position {
                settings.window.x = Some(position.x);
                settings.window.y = Some(position.y);
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(String::from)
}

pub(crate) fn snapshot(state: &AppState) -> Result<SettingsSnapshot, String> {
    // 两个计数共用一次仓库就绪校验：`ready_repository_path` 每次都做一遍轻量
    // format/version 检查，同一份快照里没必要跑两遍。
    let repository = state.ready_repository_path().ok();
    let automatic_backup_file_count = repository
        .as_deref()
        .and_then(|root| history::count_scheduled_files(root).ok());
    let idle_integrity_pending_count = repository
        .as_deref()
        .and_then(|root| history::count_pending_verify_nodes(root).ok());
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

pub(crate) fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        return Err("主题设置无效".into());
    }
    if !matches!(
        settings.automatic_backup_check_mode.as_str(),
        "quick" | "full"
    ) {
        return Err("自动备份检查方式设置无效".into());
    }
    if !matches!(settings.content.density.as_str(), "comfortable" | "compact") {
        return Err("内容密度设置无效".into());
    }
    if !matches!(
        settings.content.default_panel.as_str(),
        "overview" | "history" | "authenticity"
    ) {
        return Err("默认内容面板无效".into());
    }
    if !matches!(
        settings.pin_board.texture_cache_level.as_str(),
        "low" | "medium" | "high"
    ) {
        return Err("素材板纹理缓存等级无效".into());
    }
    if !(1.0..=200.0).contains(&settings.pin_board.arrangement_gap_px) {
        return Err("素材板阵列间距超出允许范围".into());
    }
    for (label, shortcut) in [
        ("锁定画板", &settings.pin_board.lock_shortcut),
        ("画板全屏", &settings.pin_board.fullscreen_shortcut),
    ] {
        if !shortcut_is_well_formed(shortcut) {
            return Err(format!("素材板{label}快捷键格式无效"));
        }
    }
    if settings.window.width < MIN_WINDOW_WIDTH
        || settings.window.height < MIN_WINDOW_HEIGHT
        || settings.window.width > MAX_WINDOW_WIDTH
        || settings.window.height > MAX_WINDOW_HEIGHT
    {
        return Err("窗口尺寸超出允许范围".into());
    }
    if !settings.repository_path.trim().is_empty() {
        let path = Path::new(settings.repository_path.trim());
        if !path.is_absolute() {
            return Err("作品仓库必须使用绝对目录路径".into());
        }
        if path.parent().is_none() || path.file_name().is_none() {
            return Err("不能把磁盘或文件系统根目录用作作品仓库".into());
        }
        if path.exists() && !path.is_dir() {
            return Err("作品仓库路径不是目录".into());
        }
    }
    Ok(())
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("设置文件路径无效")?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建设置目录：{error}"))?;
    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|error| format!("无法创建临时设置：{error}"))?;
    serde_json::to_writer_pretty(&mut temporary, value)
        .map_err(|error| format!("无法序列化设置：{error}"))?;
    temporary
        .write_all(b"\n")
        .map_err(|error| format!("无法写入设置：{error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步设置：{error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("无法替换设置文件：{}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{mpsc, Arc},
        thread,
        time::Duration,
    };

    use super::*;

    use crate::fuzz_support::random_bytes;

    #[test]
    fn default_settings_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let expected = AppSettings::default();
        write_json_atomic(&path, &expected).unwrap();
        let (actual, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(actual.version, CURRENT_SETTINGS_VERSION);
        assert_eq!(actual.window.width, 1320);
        assert_eq!(actual.pin_board.lock_shortcut, "CommandOrControl+R");
        assert_eq!(actual.pin_board.fullscreen_shortcut, "F11");
    }

    #[test]
    fn settings_updates_use_revision_cas_and_bump_the_counter() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let initial = AppSettings::default();
        write_json_atomic(&path, &initial).unwrap();
        let service = SettingsService::new(initial, path.clone());

        let saved = service
            .update(Some(0), |settings| {
                settings.theme = "dark".into();
                Ok(())
            })
            .unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.theme, "dark");

        let conflict = service
            .update(Some(0), |settings| {
                settings.theme = "light".into();
                Ok(())
            })
            .unwrap_err();
        assert_eq!(conflict.code, "settings_conflict");
        assert!(conflict.retryable);

        // 冲突不得覆盖文件或内存中的较新设置。
        let (loaded, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.theme, "dark");
        assert_eq!(loaded.revision, 1);
        assert_eq!(service.read().unwrap().theme, "dark");
    }

    #[test]
    fn concurrent_settings_saves_allow_exactly_one_cas_winner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let initial = AppSettings::default();
        write_json_atomic(&path, &initial).unwrap();
        let service = Arc::new(SettingsService::new(initial, path.clone()));
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let handles = (0..2)
            .map(|index| {
                let service = service.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    service
                        .update(Some(0), |settings| {
                            settings.theme = if index == 0 { "dark" } else { "light" }.into();
                            Ok(())
                        })
                        .map(|_| index)
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        let winners = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(winners, 1, "同一 revision 只能有一个写入者成功");
        let conflict = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("另一个写入者必须得到冲突");
        assert_eq!(conflict.code, "settings_conflict");

        let (loaded, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.revision, 1);
    }

    #[test]
    fn internal_patches_preserve_concurrent_fields_and_bump_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut initial = AppSettings::default();
        initial.theme = "dark".into();
        write_json_atomic(&path, &initial).unwrap();
        let service = SettingsService::new(initial, path.clone());

        // 托盘写入不携带期望 revision：直接在当前快照上打补丁，保留设置页已选的
        // 其他字段。
        let paused = service
            .update(None, |settings| {
                settings.pause_automatic_backups = true;
                Ok(())
            })
            .unwrap();
        assert!(paused.pause_automatic_backups);
        assert_eq!(paused.theme, "dark");
        assert_eq!(paused.revision, 1);

        // 基于旧 revision 的设置页保存必须得到冲突，而不是静默覆盖托盘改动。
        let conflict = service
            .update(Some(0), |settings| {
                settings.theme = "light".into();
                Ok(())
            })
            .unwrap_err();
        assert_eq!(conflict.code, "settings_conflict");
    }

    #[test]
    fn a_failed_settings_patch_leaves_memory_and_file_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let initial = AppSettings::default();
        write_json_atomic(&path, &initial).unwrap();
        let before = fs::read(&path).unwrap();
        let service = SettingsService::new(initial, path.clone());

        let error = service
            .update(Some(0), |_| {
                Err(CommandError::invalid_input("主题设置无效"))
            })
            .unwrap_err();
        assert_eq!(error.code, "invalid_input");
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(service.read().unwrap().revision, 0);
    }

    #[test]
    fn pin_board_shortcut_validation_accepts_defaults_and_rejects_empty_parts() {
        let mut settings = AppSettings::default();
        validate_settings(&settings).unwrap();

        settings.pin_board.lock_shortcut = "CommandOrControl++".into();
        assert!(validate_settings(&settings).is_err());

        settings.pin_board.lock_shortcut = String::new();
        settings.pin_board.fullscreen_shortcut = String::new();
        validate_settings(&settings).unwrap();
    }

    #[test]
    fn migrates_the_v1_pin_board_lock_default() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        let mut legacy = AppSettings::default();
        legacy.version = 1;
        legacy.pin_board.lock_shortcut = "CommandOrControl+Shift+K".into();
        write_json_atomic(&path, &legacy).unwrap();

        let (actual, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(actual.version, CURRENT_SETTINGS_VERSION);
        // v1 的旧默认键位随迁移升级为 Client 的 Ctrl+R，否则用户升级后 Ctrl+R 仍不生效。
        assert_eq!(actual.pin_board.lock_shortcut, "CommandOrControl+R");
    }

    #[test]
    fn settings_migration_preserves_a_custom_lock_shortcut() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        let mut legacy = AppSettings::default();
        legacy.version = 1;
        legacy.pin_board.lock_shortcut = "CommandOrControl+Alt+L".into();
        write_json_atomic(&path, &legacy).unwrap();

        let (actual, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(actual.version, CURRENT_SETTINGS_VERSION);
        assert_eq!(actual.pin_board.lock_shortcut, "CommandOrControl+Alt+L");
    }

    #[test]
    fn atomic_settings_write_replaces_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let initial = AppSettings::default();
        write_json_atomic(&path, &initial).unwrap();

        let mut updated = initial;
        updated.theme = "dark".into();
        updated.window.width = 1440;
        write_json_atomic(&path, &updated).unwrap();

        let (actual, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(actual.theme, "dark");
        assert_eq!(actual.window.width, 1440);
    }

    #[test]
    fn repository_save_only_initializes_a_new_selection() {
        let directory = tempfile::tempdir().unwrap();
        let missing_existing = directory.path().join("missing-existing");
        fs::create_dir(&missing_existing).unwrap();

        assert!(
            prepare_repository(Some(&missing_existing), &missing_existing.to_string_lossy())
                .is_err()
        );
        assert!(!crate::storage::database_path(&missing_existing).exists());

        let new_repository = directory.path().join("new-repository");
        prepare_repository(None, &new_repository.to_string_lossy()).unwrap();
        assert!(crate::storage::database_path(&new_repository).is_file());
    }

    #[test]
    fn repository_readiness_caches_integrity_check_and_rechecks_version() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        library::initialize(&root).unwrap();
        let mut settings = AppSettings::default();
        settings.repository_path = root.to_string_lossy().into_owned();
        let state = AppState::new(
            settings,
            directory.path().join("settings.json"),
            directory.path().join("logs"),
            None,
        );
        library::take_integrity_check_count();

        assert_eq!(state.ready_repository_path().unwrap(), root);
        assert_eq!(state.ready_repository_path().unwrap(), root);
        assert_eq!(library::take_integrity_check_count(), 1);

        crate::storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE repository_meta SET value = '99' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        let error = state.ready_repository_path().unwrap_err();
        assert!(error.contains("更新版本"), "{error}");
        assert_eq!(library::take_integrity_check_count(), 0);

        crate::storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE repository_meta SET value = '5' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        assert_eq!(state.ready_repository_path().unwrap(), root);
        assert_eq!(library::take_integrity_check_count(), 1);

        crate::storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE repository_meta SET value = 'other' WHERE key = 'format'",
                [],
            )
            .unwrap();
        let error = state.ready_repository_path().unwrap_err();
        assert!(error.contains("不是 Lilith Artworks 仓库"), "{error}");
        assert_eq!(library::take_integrity_check_count(), 0);
    }

    #[test]
    fn repository_switch_waits_for_an_active_repository_lease() {
        let directory = tempfile::tempdir().unwrap();
        let first_root = directory.path().join("first");
        let second_root = directory.path().join("second");
        library::initialize(&first_root).unwrap();
        library::initialize(&second_root).unwrap();
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
        library::initialize(&root).unwrap();
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

    #[test]
    fn an_out_of_range_window_size_keeps_the_rest_of_the_settings() {
        // 复现真实故障：窗口在最小化状态下退出时，`GetWindowRect` 返回任务栏图标
        // 矩形（约 237×39）并被写进 settings.json；此前加载会因此把整份设置回落
        // 默认，用户的仓库路径与全部偏好一起丢失。
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut broken = AppSettings::default();
        broken.repository_path = "F:\\LilithData\\Artworks".into();
        broken.pin_board.arrangement_gap_px = 3.0;
        broken.pin_board.fullscreen_shortcut = "Escape".into();
        broken.window.width = 237;
        broken.window.height = 39;
        write_json_atomic(&path, &broken).unwrap();

        let (loaded, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.repository_path, "F:\\LilithData\\Artworks");
        assert_eq!(loaded.pin_board.arrangement_gap_px, 3.0);
        assert_eq!(loaded.pin_board.fullscreen_shortcut, "Escape");
        // 越界尺寸被夹回可用范围，窗口仍然可见、可拖动。
        assert_eq!(loaded.window.width, MIN_WINDOW_WIDTH);
        assert_eq!(loaded.window.height, MIN_WINDOW_HEIGHT);
    }

    #[test]
    fn taskbar_icon_sized_rects_are_not_a_plausible_window_size() {
        assert!(!plausible_window_size(237, 39));
        assert!(!plausible_window_size(0, 0));
        assert!(!plausible_window_size(MIN_WINDOW_WIDTH - 1, 840));
        assert!(!plausible_window_size(1320, MAX_WINDOW_HEIGHT + 1));
        assert!(plausible_window_size(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
        assert!(plausible_window_size(1320, 840));
    }

    #[test]
    fn a_window_saved_on_a_disconnected_monitor_is_not_treated_as_visible() {
        let monitors = [MonitorRect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        }];
        // 仍在一块显示器内的位置。
        assert!(window_overlaps_a_monitor(&monitors, 100, 100, 1320, 840));
        // 曾经落在右侧外接屏（x≈2560），屏幕断开后已不可见。
        assert!(!window_overlaps_a_monitor(&monitors, 3400, 200, 1320, 840));
        // 最小化时写入的任务栏坐标。
        assert!(!window_overlaps_a_monitor(
            &monitors, -32000, -32000, 1320, 840
        ));
        // 只蹭到一条边：重叠不足最低可见范围，仍按不可见处理。
        assert!(!window_overlaps_a_monitor(&monitors, 2550, 100, 1320, 840));

        let side_by_side = [
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            MonitorRect {
                x: 1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
        ];
        // 位于第二块显示器时仍然可见，不会因为不在主屏而被误判。
        assert!(window_overlaps_a_monitor(
            &side_by_side,
            2000,
            100,
            1320,
            840
        ));
    }

    #[test]
    fn sanitizing_repairs_only_the_out_of_range_fields() {
        let mut settings = AppSettings::default();
        settings.repository_path = "F:\\LilithData\\Artworks".into();
        settings.theme = "neon".into();
        settings.pin_board.arrangement_gap_px = 0.0;
        settings.window.width = 237;

        assert!(sanitize_settings(&mut settings));
        assert_eq!(settings.repository_path, "F:\\LilithData\\Artworks");
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.pin_board.arrangement_gap_px, 10.0);
        assert_eq!(settings.window.width, MIN_WINDOW_WIDTH);
        // 修正后的设置已经合法，再次加载不再触发修正。
        assert!(!sanitize_settings(&mut settings));
    }

    /// 畸形输入模糊（设置文件）：任意字节都必须让 `load_settings` 干净地回落并**如实给出
    /// 警告**，绝不 panic；两条断言分别锁定「回落结果合法」与「用户文件不被悄悄覆盖」：
    /// ① 版本为当前版本时（`sanitize_settings` 会运行），返回的设置必须通过
    ///    `validate_settings` —— 这是「修正在内存里、不整份清空」的契约；
    /// ② **解析失败的设置文件必须被原样保留**：它可能是更高版本写入的，一次失败的读取
    ///    绝不能把它覆盖成默认值（那才是真正的数据丢失）。
    #[test]
    fn fuzzed_settings_files_fall_back_without_clobbering_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        let mut cases: Vec<Vec<u8>> = Vec::new();
        for seed in 0..64_u64 {
            cases.push(random_bytes(seed, (seed as usize * 17) % 512));
        }
        for text in [
            "",
            "{}",
            "[]",
            "null",
            "123",
            "\"text\"",
            "{",
            "{\"version\":}",
            "{\"version\":99}",
            "{\"version\":-1}",
            "{\"version\":4294967296}",
            "{\"theme\":\"bogus\"}",
            "{\"content\":{\"density\":\"huge\"}}",
        ] {
            cases.push(text.as_bytes().to_vec());
        }
        // 两类经典的「解析器耗尽」手法：超深嵌套与超长字面量。二者都必须被干净拒绝，
        // 而不是把栈或内存吃光。
        cases.push(format!("{}{}", "[".repeat(10_000), "]".repeat(10_000)).into_bytes());
        cases.push("9".repeat(100_000).into_bytes());
        cases.push(format!("{{\"theme\":\"{}\"}}", "x".repeat(100_000)).into_bytes());

        for (index, bytes) in cases.iter().enumerate() {
            fs::write(&path, bytes).unwrap();
            let parses = std::str::from_utf8(bytes)
                .ok()
                .is_some_and(|text| serde_json::from_str::<AppSettings>(text).is_ok());

            let (settings, warning) = load_settings(&path);

            if parses && settings.version == CURRENT_SETTINGS_VERSION {
                assert!(
                    validate_settings(&settings).is_ok(),
                    "case #{index}：当前版本的设置必须被就地修正为合法（不整份回落）"
                );
            }
            if !parses {
                assert!(
                    warning.is_some(),
                    "case #{index}：解析失败必须如实给出警告，而不是静默回落"
                );
                assert_eq!(
                    fs::read(&path).unwrap(),
                    *bytes,
                    "case #{index}：解析失败的设置文件必须被原样保留"
                );
            }
        }
    }
}
