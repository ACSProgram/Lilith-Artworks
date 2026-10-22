//! 应用设置、进程级运行状态与设置相关命令。
//!
//! 按变化原因拆成五部分：`model` 管设置文件格式与校验，`service` 管原子写入与
//! revision/CAS，`state` 管 `AppState` 与仓库租约，`window` 管窗口几何，`commands`
//! 管 Tauri 命令与仓库准备。对外仍经 `crate::app::settings::*` 访问。

pub(crate) mod commands;
mod model;
mod service;
mod state;
mod window;

pub(crate) use commands::{prepare_repository, set_automatic_backups_paused};
pub(crate) use model::{load_settings, validate_settings, AppSettings, CURRENT_SETTINGS_VERSION};
pub(crate) use state::{snapshot, AppState, SettingsSnapshot};
pub(crate) use window::{capture_window_settings, restore_window_settings};
