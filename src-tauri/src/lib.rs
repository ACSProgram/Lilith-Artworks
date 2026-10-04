mod app;
mod authenticity;
mod backup;
mod cleanup;
#[cfg(feature = "headless")]
mod headless;
mod history;
mod library;
mod pin_board;
mod storage;

use std::{
    fs::File,
    path::{Path, PathBuf},
};

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WindowEvent,
};

const BASE_TRAY_ICON_SIZE: f64 = 16.0;
/// One rotated file is 4 MiB, and five files are kept, so a diagnosis session
/// survives several restarts of a busy repository without growing unbounded.
const LOG_MAX_FILE_SIZE: u128 = 4_000_000;
const LOG_KEPT_FILES: usize = 5;
/// Upper bound for the shutdown handshake with the webview. Normal confirmation
/// (pin-board finalize plus settings flush) arrives in well under a second; the
/// timeout only covers a hung or crashed webview so the window can still close.
const SHUTDOWN_CONFIRM_TIMEOUT_MS: u64 = 15_000;

/// Operations log at Info; `LILITH_LOG_LEVEL=debug|trace` (or warn|error) turns
/// the per-step detail on or off without rebuilding the application.
fn log_level_from_env() -> log::LevelFilter {
    match std::env::var("LILITH_LOG_LEVEL")
        .unwrap_or_default()
        .as_str()
    {
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

struct RuntimeIcons {
    window: Image<'static>,
    tray: Image<'static>,
}

struct TrayMenuState {
    pause_automatic: MenuItem<tauri::Wry>,
}

fn automatic_backup_menu_text(paused: bool) -> &'static str {
    if paused {
        "继续所有自动备份"
    } else {
        "暂停所有自动备份"
    }
}

pub(crate) fn refresh_tray_backup_menu(app: &AppHandle) -> Result<(), String> {
    if let Some(menu) = app.try_state::<TrayMenuState>() {
        let paused = app.state::<app::AppState>().automatic_backups_paused();
        menu.pause_automatic
            .set_text(automatic_backup_menu_text(paused))
            .map_err(|error| format!("无法更新托盘备份菜单：{error}"))?;
    }
    Ok(())
}

fn load_runtime_icons(icon_path: &Path, tray_target_size: u32) -> Result<RuntimeIcons, String> {
    let window =
        Image::from_path(icon_path).map_err(|error| format!("无法读取应用图标：{error}"))?;
    let icon_dir = ico::IconDir::read(
        File::open(icon_path).map_err(|error| format!("无法打开应用图标：{error}"))?,
    )
    .map_err(|error| format!("无法解析应用图标：{error}"))?;
    let entry = icon_dir
        .entries()
        .iter()
        .filter(|entry| entry.width() == entry.height())
        .min_by_key(|entry| {
            (
                entry.width().abs_diff(tray_target_size),
                std::cmp::Reverse(entry.width()),
            )
        })
        .ok_or("应用图标中没有可用的正方形尺寸")?;
    let decoded = entry.decode().map_err(|error| {
        format!(
            "无法解码 {}x{} 图标帧：{error}",
            entry.width(),
            entry.height()
        )
    })?;
    Ok(RuntimeIcons {
        window,
        tray: Image::new_owned(
            decoded.rgba_data().to_vec(),
            decoded.width(),
            decoded.height(),
        ),
    })
}

fn tray_target_size(application: &tauri::App) -> u32 {
    let scale = application
        .get_webview_window("main")
        .and_then(|window| window.scale_factor().ok())
        .unwrap_or(1.0);
    (BASE_TRAY_ICON_SIZE * scale)
        .round()
        .clamp(BASE_TRAY_ICON_SIZE, 64.0) as u32
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Starts the shutdown handshake with the webview: the frontend is expected to
/// finalize the pin board (persist and truncate the step history) and then call
/// `confirm_app_shutdown`. A fallback thread force-exits when the confirmation
/// never arrives, so a hung webview cannot keep the application alive.
fn begin_webview_shutdown(app: &AppHandle) {
    if let Err(error) = app.emit("app_shutdown_requested", ()) {
        log::error!("failed to emit shutdown request: {error}");
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(
            SHUTDOWN_CONFIRM_TIMEOUT_MS,
        ));
        let state = handle.state::<app::AppState>();
        if state.shutdown_confirmed() || state.exit_requested() {
            return;
        }
        log::warn!("webview shutdown confirmation timed out; forcing exit");
        handle.exit(0);
    });
}

/// Confirmation side of the shutdown handshake. Runs the same irreversible
/// sequence the previous close paths ran inline: wait for the shared operation
/// lock, stop the scheduler threads, then exit the process.
#[tauri::command]
async fn confirm_app_shutdown(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<app::AppState>();
    if !state.mark_shutdown_confirmed() {
        // Another confirmation is already running the exit sequence.
        return Ok(());
    }
    log::info!("Lilith Artworks shutting down (webview confirmed)");
    app.state::<backup::BackupState>().shutdown();
    state.request_exit();
    app.exit(0);
    Ok(())
}

fn build_tray(application: &tauri::App, runtime_icon: Option<Image<'static>>) -> tauri::Result<()> {
    let show = MenuItem::with_id(
        application,
        "show",
        "打开 Lilith Artworks",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(application, "quit", "退出", true, None::<&str>)?;
    let pause = MenuItem::with_id(
        application,
        "pause-automatic",
        automatic_backup_menu_text(
            application
                .state::<app::AppState>()
                .automatic_backups_paused(),
        ),
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(application, &[&show, &pause, &quit])?;
    application.manage(TrayMenuState {
        pause_automatic: pause.clone(),
    });
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Lilith Artworks")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "pause-automatic" => {
                let paused = app.state::<app::AppState>().automatic_backups_paused();
                if let Err(error) = app::settings::set_automatic_backups_paused(app, !paused) {
                    log::error!("failed to update automatic backup pause state: {error}");
                }
            }
            "quit" => {
                let _ = app::capture_window_settings(app);
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
                if app.state::<app::AppState>().begin_shutdown_handshake() {
                    log::info!("Lilith Artworks shutting down");
                    begin_webview_shutdown(app);
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = runtime_icon {
        builder = builder.icon(icon);
    } else if let Some(icon) = application.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(application)?;
    Ok(())
}

/// 无头命令行入口（`feature = "headless"`）。规格与定位见 `src/headless.rs`；
/// 它不进发布产物，只供发布前的压力测试以独立进程调用真实可执行文件。
#[cfg(feature = "headless")]
pub fn run_headless(args: &[String]) -> i32 {
    headless::run(args)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
                .max_file_size(LOG_MAX_FILE_SIZE)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(LOG_KEPT_FILES))
                .level(log_level_from_env())
                .build(),
        )
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|application| {
            let started = std::time::Instant::now();
            log::info!(
                "Lilith Artworks starting: version={}, log_level={}",
                env!("CARGO_PKG_VERSION"),
                log_level_from_env()
            );
            let config_directory = application
                .path()
                .app_config_dir()
                .map_err(|error| format!("无法定位应用设置目录：{error}"))?;
            let settings_path: PathBuf = config_directory.join("settings.json");
            let log_directory = application
                .path()
                .app_log_dir()
                .map_err(|error| format!("无法定位应用日志目录：{error}"))?;
            let (settings, warning) = app::load_settings(&settings_path);
            application.manage(app::AppState::new(
                settings,
                settings_path,
                log_directory,
                warning,
            ));
            let backup_state = backup::BackupState::default();
            backup_state.start_scheduler(application.handle().clone())?;
            application.manage(backup_state);
            let resource_dir = application.path().resource_dir()?;
            let model_candidates = [
                resource_dir.join("resources").join("models"),
                resource_dir.join("models"),
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("resources")
                    .join("models"),
            ];
            let models_dir = model_candidates
                .into_iter()
                .find(|path| {
                    path.join("encoder_Q.onnx").is_file() && path.join("decoder_Q.onnx").is_file()
                })
                .unwrap_or_else(|| resource_dir.join("resources").join("models"));
            application.manage(authenticity::AuthenticityState::new(models_dir));
            app::restore_window_settings(application.handle())?;
            let executable =
                std::env::current_exe().map_err(|error| format!("无法获取程序路径：{error}"))?;
            let icon_path = executable
                .parent()
                .ok_or("程序目录无效")?
                .join("resources")
                .join("icon.ico");
            let runtime_icon = load_runtime_icons(&icon_path, tray_target_size(application)).ok();
            if let (Some(window), Some(icons)) = (
                application.get_webview_window("main"),
                runtime_icon.as_ref(),
            ) {
                let _ = window.set_icon(icons.window.clone());
            }
            build_tray(application, runtime_icon.map(|icons| icons.tray))?;
            log::info!(
                "Lilith Artworks started: elapsed_ms={}",
                started.elapsed().as_millis()
            );
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                let state = window.state::<app::AppState>();
                if state.exit_requested() {
                    return;
                }
                if let Err(error) = app::capture_window_settings(window.app_handle()) {
                    log::error!("failed to persist window settings: {error}");
                }
                if state.close_to_tray() {
                    api.prevent_close();
                    let _ = window.hide();
                } else if state.begin_shutdown_handshake() {
                    log::info!("Lilith Artworks shutting down");
                    api.prevent_close();
                    // Hiding first also fires the webview visibilitychange
                    // handler, which saves open pin boards as a side effect.
                    let _ = window.hide();
                    begin_webview_shutdown(window.app_handle());
                } else {
                    // A handshake is already running; just keep the window shut.
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            confirm_app_shutdown,
            app::settings::get_app_settings,
            app::settings::save_app_settings,
            app::settings::open_log_directory,
            app::settings::open_legal_directory,
            app::settings::open_settings_directory,
            app::settings::reveal_path_in_folder,
            app::cleanup_commands::list_pending_file_cleanup,
            app::cleanup_commands::retry_pending_file_cleanup,
            app::cleanup_commands::scan_repository_unreferenced,
            app::cleanup_commands::cleanup_repository_unreferenced,
            app::workflows::acknowledge_backup_disable_notices,
            app::workflows::get_backup_disable_notice_target,
            app::workflows::scrub_repository_integrity,
            app::workflows::create_repository_backup,
            library::get_repository_status,
            library::list_library_tree,
            library::search_library,
            library::create_library_group,
            app::workflows::create_library_artwork,
            library::rename_library_node,
            library::trash_library_nodes,
            library::list_library_trash,
            library::restore_library_trash,
            app::workflows::permanently_delete_library_trash,
            app::workflows::empty_library_trash,
            library::move_library_nodes,
            history::get_artwork_history,
            app::workflows::fork_artwork_branch,
            app::workflows::update_artwork_branch,
            history::rename_history_node,
            app::workflows::delete_artwork_branch,
            backup::run_branch_backup,
            backup::restore_history_node,
            backup::compact_history_node,
            backup::delete_history_subtree,
            backup::set_history_checkpoint,
            backup::reverify_branch_history,
            backup::get_backup_runtime_status,
            backup::cancel_backup_operation,
            app::workflows::enter_branch_publication,
            authenticity::get_branch_publication,
            app::workflows::cancel_branch_publication,
            app::workflows::publish_branch_artifact,
            authenticity::decode_authenticity,
            authenticity::search_certification_records,
            authenticity::preview_authenticity_image,
            authenticity::preview_branch_artifact,
            authenticity::preview_branch_artifact_output,
            authenticity::cancel_authenticity_operation,
            authenticity::preview_certification_record,
            authenticity::export_certification_record,
            authenticity::estimate_authenticity_output_size,
            pin_board::list_pin_boards,
            pin_board::list_pin_board_trash,
            pin_board::create_pin_board,
            pin_board::rename_pin_board,
            pin_board::trash_pin_board,
            pin_board::restore_pin_board,
            pin_board::reorder_pin_boards,
            pin_board::delete_pin_board_permanently,
            pin_board::empty_pin_board_trash,
            pin_board::load_pin_board,
            pin_board::save_pin_board,
            pin_board::finalize_pin_board,
            pin_board::paste_pin_board_images,
            pin_board::import_pin_board_images,
            pin_board::import_pin_board_clipboard_image,
            pin_board::export_pin_board_images,
            pin_board::read_pin_board_image_png,
            pin_board::read_pin_board_texture,
            pin_board::read_pin_board_clipboard_paths,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lilith Artworks");
}
