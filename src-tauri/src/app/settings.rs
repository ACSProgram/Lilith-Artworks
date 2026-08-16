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

use crate::backup::BackupState;
use crate::{history, library};

const CURRENT_SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AppSettings {
    version: u32,
    repository_path: String,
    theme: String,
    close_to_tray: bool,
    pause_automatic_backups: bool,
    window: WindowSettings,
    content: ContentSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: CURRENT_SETTINGS_VERSION,
            repository_path: String::new(),
            theme: "system".into(),
            close_to_tray: true,
            pause_automatic_backups: false,
            window: WindowSettings::default(),
            content: ContentSettings::default(),
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
    repository_operation: Arc<Mutex<()>>,
    exit_requested: Arc<AtomicBool>,
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
            exit_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn repository_path(&self) -> Result<Option<PathBuf>, String> {
        let settings = self.settings.read().map_err(|_| "设置状态已损坏")?;
        let value = settings.repository_path.trim();
        Ok((!value.is_empty()).then(|| PathBuf::from(value)))
    }

    pub(crate) fn ready_repository_path(&self) -> Result<PathBuf, String> {
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
        library::open_existing(&root)?;
        *validated = Some(root.clone());
        Ok(root)
    }

    pub(crate) fn with_ready_repository<T>(
        &self,
        operation: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        let _lease = self
            .repository_operation
            .lock()
            .map_err(|_| "仓库操作锁已损坏")?;
        let root = self.ready_repository_path()?;
        operation(&root)
    }

    fn with_repository_switch<T>(
        &self,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _lease = self
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

    pub(crate) fn request_exit(&self) {
        self.exit_requested.store(true, Ordering::SeqCst);
    }

    pub(crate) fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::SeqCst)
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
            Ok(settings) if settings.version == CURRENT_SETTINGS_VERSION => {
                match validate_settings(&settings) {
                    Ok(()) => (settings, None),
                    Err(error) => (
                        AppSettings::default(),
                        Some(format!("设置内容无效，已临时使用默认值：{error}")),
                    ),
                }
            }
            Ok(settings) if settings.version > CURRENT_SETTINGS_VERSION => (
                settings,
                Some("settings.json 来自更高版本，本次运行不会覆盖它".into()),
            ),
            Ok(mut settings) => {
                settings.version = CURRENT_SETTINGS_VERSION;
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
pub(crate) fn get_app_settings(state: State<'_, AppState>) -> Result<SettingsSnapshot, String> {
    snapshot(state.inner())
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

fn open_directory(path: &Path, label: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("无法创建{label}目录：{error}"))?;
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
    let next = backup_state.run_exclusive(None, || {
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
            snapshot(state.inner())
        })
    })?;
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

pub(crate) fn restore_window_settings(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let settings = state.settings.read().map_err(|_| "设置状态已损坏")?;
    let window = app.get_webview_window("main").ok_or("找不到主窗口")?;
    window
        .set_size(PhysicalSize::new(
            settings.window.width,
            settings.window.height,
        ))
        .map_err(|error| format!("无法恢复窗口大小：{error}"))?;
    if let (Some(x), Some(y)) = (settings.window.x, settings.window.y) {
        window
            .set_position(PhysicalPosition::new(x, y))
            .map_err(|error| format!("无法恢复窗口位置：{error}"))?;
    } else {
        window
            .center()
            .map_err(|error| format!("无法居中主窗口：{error}"))?;
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
    let size = (!maximized).then(|| window.outer_size().ok()).flatten();
    let position = (!maximized).then(|| window.outer_position().ok()).flatten();
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
    if !matches!(settings.content.density.as_str(), "comfortable" | "compact") {
        return Err("内容密度设置无效".into());
    }
    if !matches!(
        settings.content.default_panel.as_str(),
        "overview" | "history" | "authenticity"
    ) {
        return Err("默认内容面板无效".into());
    }
    if settings.window.width < 760
        || settings.window.height < 560
        || settings.window.width > 16_384
        || settings.window.height > 16_384
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
                "UPDATE repository_meta SET value = '9' WHERE key = 'schema_version'",
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
}
