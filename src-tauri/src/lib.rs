mod app;
mod authenticity;
mod backup;
mod history;
mod library;
mod storage;

use std::{
    fs::File,
    path::{Path, PathBuf},
};

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WindowEvent,
};

const BASE_TRAY_ICON_SIZE: f64 = 16.0;

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
                    eprintln!("failed to update automatic backup pause state: {error}");
                }
            }
            "quit" => {
                let _ = app::capture_window_settings(app);
                app.state::<backup::BackupState>().shutdown();
                app.state::<app::AppState>().request_exit();
                app.exit(0);
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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|application| {
            let config_directory = application
                .path()
                .app_config_dir()
                .map_err(|error| format!("无法定位应用设置目录：{error}"))?;
            let settings_path: PathBuf = config_directory.join("settings.json");
            let (settings, warning) = app::load_settings(&settings_path);
            application.manage(app::AppState::new(settings, settings_path, warning));
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
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                let state = window.state::<app::AppState>();
                if state.exit_requested() {
                    return;
                }
                if let Err(error) = app::capture_window_settings(window.app_handle()) {
                    eprintln!("failed to persist window settings: {error}");
                }
                if state.close_to_tray() {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    window.state::<backup::BackupState>().shutdown();
                    state.request_exit();
                    window.app_handle().exit(0);
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            app::settings::get_app_settings,
            app::settings::save_app_settings,
            library::get_repository_status,
            library::list_library_tree,
            library::search_library,
            library::create_library_group,
            library::create_library_artwork,
            library::rename_library_node,
            library::trash_library_nodes,
            library::list_library_trash,
            library::restore_library_trash,
            library::permanently_delete_library_trash,
            library::empty_library_trash,
            library::move_library_nodes,
            history::get_artwork_history,
            history::fork_artwork_branch,
            history::update_artwork_branch,
            history::rename_history_node,
            history::delete_artwork_branch,
            backup::run_branch_backup,
            backup::restore_history_node,
            backup::compact_history_node,
            backup::delete_history_subtree,
            backup::set_history_checkpoint,
            backup::get_backup_runtime_status,
            backup::cancel_backup_operation,
            authenticity::enter_branch_publication,
            authenticity::get_branch_publication,
            authenticity::publish_branch_artifact,
            authenticity::decode_authenticity,
            authenticity::search_certification_records,
            authenticity::preview_authenticity_image,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lilith Artworks");
}
