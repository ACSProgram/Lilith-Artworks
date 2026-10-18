pub(crate) mod cleanup_commands;
pub(crate) mod command_registry;
pub(crate) mod command_runtime;
pub(crate) mod diagnostics;
pub(crate) mod settings;
pub(crate) mod usecases;
pub(crate) mod workflows;

pub(crate) use command_runtime::OperationCoordinator;
pub(crate) use settings::{
    capture_window_settings, load_settings, restore_window_settings, AppState,
};
