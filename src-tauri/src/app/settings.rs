use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, State};
use tempfile::NamedTempFile;

use super::diagnostics::{self, DiagnosticsState};
use crate::backup::{BackupState, BackupTaskKind};
use crate::{history, library};

const CURRENT_SETTINGS_VERSION: u32 = 2;

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
    version: u32,
    repository_path: String,
    theme: String,
    close_to_tray: bool,
    pause_automatic_backups: bool,
    automatic_backup_check_mode: String,
    window: WindowSettings,
    content: ContentSettings,
    pin_board: PinBoardSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: CURRENT_SETTINGS_VERSION,
            repository_path: String::new(),
            theme: "system".into(),
            close_to_tray: true,
            pause_automatic_backups: false,
            automatic_backup_check_mode: "quick".into(),
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

#[derive(Clone)]
pub(crate) struct AppState {
    settings: Arc<RwLock<AppSettings>>,
    settings_path: PathBuf,
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
        Self {
            settings: Arc::new(RwLock::new(settings)),
            settings_path,
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
    /// 调用依赖它们的命令。
    #[cfg(feature = "headless")]
    pub(crate) fn for_headless_repository(repository: &Path, state_directory: &Path) -> Self {
        let mut settings = AppSettings::default();
        settings.repository_path = repository.to_string_lossy().into_owned();
        Self::new(
            settings,
            state_directory.join("settings.json"),
            state_directory.join("logs"),
            None,
        )
    }

    pub(crate) fn repository_path(&self) -> Result<Option<PathBuf>, String> {
        let settings = self.settings.read().map_err(|_| "设置状态已损坏")?;
        let value = settings.repository_path.trim();
        Ok((!value.is_empty()).then(|| PathBuf::from(value)))
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
        let wait_started = std::time::Instant::now();
        let _lease = self.repository_lease.read().map_err(|_| "仓库租约已损坏")?;
        let _operation = self
            .repository_operation
            .lock()
            .map_err(|_| "仓库操作锁已损坏")?;
        // 只在真的等待时才记录：用于判断前端卡死是否伴随仓库锁争用。
        let waited_ms = wait_started.elapsed().as_millis();
        if waited_ms > LOCK_WAIT_WARN_MS {
            log::warn!("[slow] repository mutation lock waited {waited_ms} ms");
        }
        let root = self.ready_repository_path()?;
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
        // 空标签不打印括号，避免日志里出现无意义的 `()`。
        let suffix = if label.is_empty() {
            String::new()
        } else {
            format!(" [{label}]")
        };
        let wait_started = std::time::Instant::now();
        let _lease = self.repository_lease.read().map_err(|_| "仓库租约已损坏")?;
        let waited_ms = wait_started.elapsed().as_millis();
        if waited_ms > LOCK_WAIT_WARN_MS {
            log::warn!("[slow] repository read lease waited {waited_ms} ms{suffix}");
        }
        let root = self.ready_repository_path()?;
        let ran_started = std::time::Instant::now();
        let result = operation(&root);
        let ran_ms = ran_started.elapsed().as_millis();
        if ran_ms > READ_HOLD_WARN_MS {
            log::warn!("[slow] repository read took {ran_ms} ms{suffix}");
        }
        result
    }

    fn with_repository_switch<T>(
        &self,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _lease = self
            .repository_lease
            .write()
            .map_err(|_| "仓库租约已损坏")?;
        let _operation = self
            .repository_operation
            .lock()
            .map_err(|_| "仓库操作锁已损坏")?;
        operation()
    }

    fn set_validated_repository(&self, root: Option<PathBuf>) -> Result<(), String> {
        *self
            .validated_repository
            .lock()
            .map_err(|_| "仓库校验状态已损坏")? = root;
        Ok(())
    }

    pub(crate) fn close_to_tray(&self) -> bool {
        self.settings
            .read()
            .map(|settings| settings.close_to_tray)
            .unwrap_or(true)
    }

    pub(crate) fn automatic_backups_paused(&self) -> bool {
        self.settings
            .read()
            .map(|settings| settings.pause_automatic_backups)
            .unwrap_or(false)
    }

    /// 自动备份默认检查方式是否为快速。仅在设置显式选择 full 时返回 false；
    /// 设置缺失或状态损坏时保持默认的快速检查。
    pub(crate) fn automatic_backup_quick_default(&self) -> bool {
        self.settings
            .read()
            .map(|settings| settings.automatic_backup_check_mode.as_str() != "full")
            .unwrap_or(true)
    }

    /// 素材板原生纹理解码结果缓存等级（low/medium/high）。
    pub(crate) fn pin_board_texture_cache_level(&self) -> String {
        self.settings
            .read()
            .map(|settings| settings.pin_board.texture_cache_level.clone())
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
    settings: AppSettings,
    settings_path: String,
    log_directory: String,
    warning: Option<String>,
    automatic_backup_file_count: Option<usize>,
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
    state: State<'_, AppState>,
) -> Result<SettingsSnapshot, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || snapshot(&state))
        .await
        .map_err(|error| format!("设置读取任务异常结束：{error}"))?
}

#[tauri::command]
pub(crate) fn open_log_directory(state: State<'_, AppState>) -> Result<(), String> {
    open_directory(&state.log_directory, "日志")
}

#[tauri::command]
pub(crate) fn open_settings_directory(state: State<'_, AppState>) -> Result<(), String> {
    let directory = state.settings_path.parent().ok_or("设置文件路径无效")?;
    open_directory(directory, "设置")
}

/// 在系统文件管理器中打开给定文件所在的文件夹（Windows 上会选中该文件）。
#[tauri::command]
pub(crate) fn reveal_path_in_folder(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("文件路径为空".into());
    }
    let path = Path::new(trimmed);
    if !path.is_file() {
        return Err("文件不存在或已被移动".into());
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map_err(|error| format!("无法打开文件所在文件夹：{error}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map_err(|error| format!("无法打开文件所在文件夹：{error}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let directory = path.parent().ok_or("文件路径无效")?;
        Command::new("xdg-open")
            .arg(directory)
            .spawn()
            .map_err(|error| format!("无法打开文件所在文件夹：{error}"))?;
    }
    Ok(())
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

fn open_directory(path: &Path, label: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("无法创建{label}目录：{error}"))?;
    open_existing_directory(path, label)
}

fn open_existing_directory(path: &Path, label: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "windows") {
        Command::new("explorer.exe")
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(path)
        .spawn()
        .map_err(|error| format!("无法打开{label}目录：{error}"))?;
    Ok(())
}

#[tauri::command]
pub(crate) fn save_app_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    settings: AppSettings,
) -> Result<SettingsSnapshot, String> {
    validate_settings(&settings)?;
    if settings.version != CURRENT_SETTINGS_VERSION {
        return Err("设置版本不受支持".into());
    }
    let paused = settings.pause_automatic_backups;
    let repository_changed = state
        .repository_path()?
        .map(|path| path.to_string_lossy().into_owned())
        != Some(settings.repository_path.trim().to_owned());
    let save = || {
        state.with_repository_switch(|| {
            let current_repository = state.repository_path()?;
            let prepared_repository = prepare_repository(
                current_repository.as_deref(),
                settings.repository_path.trim(),
            )?;
            write_json_atomic(&state.settings_path, &settings)?;
            *state.settings.write().map_err(|_| "设置状态已损坏")? = settings;
            *state.warning.write().map_err(|_| "设置警告状态已损坏")? = None;
            state.set_validated_repository(prepared_repository)?;
            log::info!(
                "settings saved: repository_changed={repository_changed}, repository_path={}",
                state
                    .repository_path()?
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            );
            snapshot(state.inner())
        })
    };
    // 切换仓库是前台长命令：登记前台等待并请求后台任务让位。普通设置保存持有
    // 运行锁的时间极短，不必打断正在运行的自动备份。
    let next = if repository_changed {
        backup_state.run_foreground(None, save)?
    } else {
        backup_state.run_exclusive(None, BackupTaskKind::UserOperation, save)?
    };
    backup_state.set_automatic_scheduling(!paused);
    backup_state.wake_scheduler();
    crate::refresh_tray_backup_menu(&app)?;
    Ok(next)
}

fn prepare_repository(
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
    Ok(Some(root.to_path_buf()))
}

pub(crate) fn set_automatic_backups_paused(app: &AppHandle, paused: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut settings = state.settings.read().map_err(|_| "设置状态已损坏")?.clone();
    settings.pause_automatic_backups = paused;
    write_json_atomic(&state.settings_path, &settings)?;
    *state.settings.write().map_err(|_| "设置状态已损坏")? = settings;
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
    let settings = state.settings.read().map_err(|_| "设置状态已损坏")?;
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
        None => {
            let settings = state.settings.read().map_err(|_| "设置状态已损坏")?;
            (settings.window.width, settings.window.height)
        }
    };
    let position = inspectable
        .then(|| window.outer_position().ok())
        .flatten()
        .filter(|position| {
            saved_position_is_on_a_current_monitor(&window, position.x, position.y, width, height)
        });

    let settings = {
        let mut settings = state.settings.write().map_err(|_| "设置状态已损坏")?;
        settings.window.maximized = maximized;
        if let Some(size) = size {
            settings.window.width = size.width;
            settings.window.height = size.height;
        }
        if let Some(position) = position {
            settings.window.x = Some(position.x);
            settings.window.y = Some(position.y);
        }
        settings.clone()
    };
    write_json_atomic(&state.settings_path, &settings)
}

fn snapshot(state: &AppState) -> Result<SettingsSnapshot, String> {
    let automatic_backup_file_count = state
        .ready_repository_path()
        .ok()
        .and_then(|root| history::count_scheduled_files(&root).ok());
    Ok(SettingsSnapshot {
        settings: state.settings.read().map_err(|_| "设置状态已损坏")?.clone(),
        settings_path: state.settings_path.to_string_lossy().into_owned(),
        log_directory: state.log_directory.to_string_lossy().into_owned(),
        warning: state
            .warning
            .read()
            .map_err(|_| "设置警告状态已损坏")?
            .clone(),
        automatic_backup_file_count,
    })
}

fn validate_settings(settings: &AppSettings) -> Result<(), String> {
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

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
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
    use std::{sync::mpsc, thread, time::Duration};

    use super::*;

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
        assert!(error.contains("版本不受支持"), "{error}");
        assert_eq!(library::take_integrity_check_count(), 0);

        crate::storage::open(&root)
            .unwrap()
            .execute(
                "UPDATE repository_meta SET value = '4' WHERE key = 'schema_version'",
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
                    switching.settings.write().unwrap().repository_path =
                        second_root.to_string_lossy().into_owned();
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
}
