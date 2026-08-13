use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        RwLock,
    },
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, State};
use tempfile::NamedTempFile;

use crate::backup::BackupState;
use crate::library;

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

pub(crate) struct AppState {
    settings: RwLock<AppSettings>,
    settings_path: PathBuf,
    warning: RwLock<Option<String>>,
    exit_requested: AtomicBool,
}

impl AppState {
    pub(crate) fn new(
        settings: AppSettings,
        settings_path: PathBuf,
        warning: Option<String>,
    ) -> Self {
        Self {
            settings: RwLock::new(settings),
            settings_path,
            warning: RwLock::new(warning),
            exit_requested: AtomicBool::new(false),
        }
    }

    pub(crate) fn repository_path(&self) -> Result<Option<PathBuf>, String> {
        let settings = self.settings.read().map_err(|_| "设置状态已损坏")?;
        let value = settings.repository_path.trim();
        Ok((!value.is_empty()).then(|| PathBuf::from(value)))
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
    warning: Option<String>,
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
pub(crate) fn save_app_settings(
    state: State<'_, AppState>,
    backup_state: State<'_, BackupState>,
    settings: AppSettings,
) -> Result<SettingsSnapshot, String> {
    validate_settings(&settings)?;
    if settings.version != CURRENT_SETTINGS_VERSION {
        return Err("设置版本不受支持".into());
    }
    if !settings.repository_path.trim().is_empty() {
        library::initialize(Path::new(settings.repository_path.trim()))?;
    }
    let paused = settings.pause_automatic_backups;
    write_json_atomic(&state.settings_path, &settings)?;
    *state.settings.write().map_err(|_| "设置状态已损坏")? = settings;
    *state.warning.write().map_err(|_| "设置警告状态已损坏")? = None;
    backup_state.set_automatic_scheduling(!paused);
    backup_state.wake_scheduler();
    snapshot(state.inner())
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
    Ok(SettingsSnapshot {
        settings: state.settings.read().map_err(|_| "设置状态已损坏")?.clone(),
        settings_path: state.settings_path.to_string_lossy().into_owned(),
        warning: state
            .warning
            .read()
            .map_err(|_| "设置警告状态已损坏")?
            .clone(),
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
}
