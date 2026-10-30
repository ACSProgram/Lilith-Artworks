mod app;
mod authenticity;
mod backup;
mod cancel;
mod cleanup;
mod contracts;
mod format;
/// 单元测试共享的确定性畸形输入生成器（只在 `cfg(test)` 下编译，不进任何产物）。
#[cfg(test)]
mod fuzz_support;
#[cfg(feature = "headless")]
mod headless;
mod history;
mod library;
mod path_scope;
mod pin_board;
mod platform;
mod preview;
mod repository_lock;
mod storage;
mod storage_overview;

use std::path::{Path, PathBuf};

use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
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

struct RuntimeIcons {
    window: Image<'static>,
    tray: Image<'static>,
}

const TRAY_MENU_ID_SHOW: &str = "show";
/// 正向语义的标识：这一项表达「自动备份是否在运行」，不是「是否已暂停」。
/// 持久化字段仍叫 `pause_automatic_backups`，两者不要互相推断。
const TRAY_MENU_ID_AUTOMATIC_BACKUPS: &str = "automatic-backups";
const TRAY_MENU_ID_QUIT: &str = "quit";

struct TrayMenuState {
    automatic_backups: CheckMenuItem<tauri::Wry>,
}

/// 把持久化的暂停状态同步到托盘的勾选项：勾选表示自动备份正在运行。
///
/// 无托盘的环境（无头入口）没有注册 `TrayMenuState`，此处静默跳过。
pub(crate) fn refresh_tray_backup_menu(app: &AppHandle) -> Result<(), String> {
    let Some(menu) = app.try_state::<TrayMenuState>() else {
        return Ok(());
    };
    let paused = app.state::<app::AppState>().automatic_backups_paused();
    menu.automatic_backups
        .set_checked(!paused)
        .map_err(|error| format!("无法更新托盘备份菜单：{error}"))
}

/// 读取随包的 `icon.ico`：窗口图标用整份文件，托盘图标按当前显示缩放挑最接近目标
/// 尺寸的正方形帧，不把挑选留给系统去拉伸邻近尺寸。
fn load_runtime_icons(icon_path: &Path, tray_target_size: u32) -> Result<RuntimeIcons, String> {
    let bytes = std::fs::read(icon_path).map_err(|error| format!("无法读取应用图标：{error}"))?;
    let window = Image::from_bytes(&bytes).map_err(|error| format!("无法解码应用图标：{error}"))?;
    let icon_dir = ico::IconDir::read(std::io::Cursor::new(&bytes))
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
        sync_window_idle(app);
    }
}

/// 把主窗口当前是否隐藏/最小化同步给调度器。
///
/// 只在主线程的窗口事件与显示入口调用：后台调度线程只读取原子值，避免跨线程
/// 查询窗口状态，也避免退出时与窗口事件循环互相等待。窗口拖动与焦点切换会大量
/// 触发这些事件，因此只在状态真正变化时唤醒调度器，避免反复打断它的等待。
fn sync_window_idle(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let idle = !window.is_visible().unwrap_or(true) || window.is_minimized().unwrap_or(false);
        let backup = app.state::<backup::BackupState>();
        if backup.set_window_idle(idle) {
            backup.wake_scheduler();
        }
    }
}

/// 隐藏主窗口后立即把窗口状态置为「空闲」并唤醒调度器。
///
/// 不重新查询可见性：`hide()` 之后 `is_visible()` 未必立刻反映新状态，而这条路径
/// 的意图是明确的。唤醒是必要的——并非所有平台在隐藏时都会发出窗口尺寸事件，
/// 否则空闲检查要等到下一次常规轮询才看到窗口已隐藏。
fn mark_main_window_hidden(app: &AppHandle) {
    let backup = app.state::<backup::BackupState>();
    if backup.set_window_idle(true) {
        backup.wake_scheduler();
    }
}

/// Starts the shutdown handshake with the webview: the frontend is expected to
/// finalize the pin board (persist and truncate the step history) and then call
/// `confirm_app_shutdown`. A fallback thread force-exits when the confirmation
/// never arrives, so a hung webview cannot keep the application alive.
fn begin_webview_shutdown(app: &AppHandle) {
    let started = std::time::Instant::now();
    log::info!("shutdown handshake: emitting app_shutdown_requested");
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
        log::warn!(
            "[watchdog] webview shutdown confirmation timed out: timeout_ms={}, elapsed_ms={}; forcing exit",
            SHUTDOWN_CONFIRM_TIMEOUT_MS,
            started.elapsed().as_millis()
        );
        handle.exit(0);
    });
}

/// 退出确认后，共享运行锁可能仍被一个长命令持有。确认路径必须等它清理完临时
/// 文件与事务，这里只把等待过程写进结构化诊断，不改变「等锁再退出」的语义。
fn start_shutdown_wait_watchdog(backup: backup::BackupState) -> std::sync::mpsc::Sender<()> {
    use std::time::Duration;

    let (stop, stopped) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut next_report = Duration::from_secs(2);
        loop {
            match stopped.recv_timeout(Duration::from_millis(500)) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let waited = started.elapsed();
                    if waited < next_report {
                        continue;
                    }
                    match backup.status() {
                        Ok(status) if status.busy => log::warn!(
                            "[watchdog] shutdown waiting for in-flight operation: waited_ms={}, operation={}, task_kind={:?}, branch={}",
                            waited.as_millis(),
                            status.operation.as_deref().unwrap_or(""),
                            status.task_kind,
                            status.active_branch_id.as_deref().unwrap_or("")
                        ),
                        Ok(_) => log::warn!(
                            "[watchdog] shutdown waiting for repository operation lock: waited_ms={}",
                            waited.as_millis()
                        ),
                        Err(error) => log::warn!(
                            "[watchdog] shutdown could not read runtime status: waited_ms={}, error={error}",
                            waited.as_millis()
                        ),
                    }
                    next_report += Duration::from_secs(5);
                }
            }
        }
    });
    stop
}

/// Confirmation side of the shutdown handshake. Runs the same irreversible
/// sequence the previous close paths ran inline: wait for the shared operation
/// lock, stop the scheduler threads, then exit the process.
#[tauri::command]
async fn confirm_app_shutdown(app: tauri::AppHandle) -> Result<(), String> {
    log::info!("confirm_app_shutdown invoked");
    let state = app.state::<app::AppState>();
    if !state.mark_shutdown_confirmed() {
        // Another confirmation is already running the exit sequence.
        return Ok(());
    }
    log::info!("Lilith Artworks shutting down (webview confirmed)");
    state.request_exit();
    let backup = app.state::<backup::BackupState>().inner().clone();
    let stop_watchdog = start_shutdown_wait_watchdog(backup.clone());
    let wait_started = std::time::Instant::now();
    backup.shutdown();
    let _ = stop_watchdog.send(());
    log::info!(
        "shutdown sequence finished: waited_ms={}",
        wait_started.elapsed().as_millis()
    );
    app.exit(0);
    Ok(())
}

fn build_tray(application: &tauri::App, runtime_icon: Option<Image<'static>>) -> tauri::Result<()> {
    // 托盘图标本身已经说明这是哪个程序，再写一遍名字只是噪音；这一项做的是把
    // 已存在的窗口调回前台，所以用「显示窗口」而不是「打开」。
    let show = MenuItem::with_id(
        application,
        TRAY_MENU_ID_SHOW,
        "显示窗口",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(application, TRAY_MENU_ID_QUIT, "退出", true, None::<&str>)?;
    // 用勾选表达状态，不写「暂停 / 继续」两套动词文案：文案恒为「自动备份」，
    // 勾上表示正在运行，取消勾选表示暂停。读状态只需读勾选值，不必解析句子。
    let automatic_backups = CheckMenuItem::with_id(
        application,
        TRAY_MENU_ID_AUTOMATIC_BACKUPS,
        "自动备份",
        true,
        !application
            .state::<app::AppState>()
            .automatic_backups_paused(),
        None::<&str>,
    )?;
    // 分隔线把日常切换和「退出」分开：误点前两项的代价是切换一次状态，误点退出
    // 会关掉程序，两者不该贴在一起。
    let separator = PredefinedMenuItem::separator(application)?;
    let menu = Menu::with_items(application, &[&show, &automatic_backups, &separator, &quit])?;
    application.manage(TrayMenuState {
        automatic_backups: automatic_backups.clone(),
    });
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Lilith Artworks")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            TRAY_MENU_ID_SHOW => show_main_window(app),
            TRAY_MENU_ID_AUTOMATIC_BACKUPS => {
                let paused = app.state::<app::AppState>().automatic_backups_paused();
                if let Err(error) = app::settings::set_automatic_backups_paused(app, !paused) {
                    log::error!("failed to update automatic backup pause state: {error}");
                    // 写入失败时持久化状态没变，但勾选项点击时已经自行翻转了勾选，
                    // 这里把它拉回真实状态，免得菜单显示一个没落盘的值。成功路径
                    // 由 `set_automatic_backups_paused` 内部刷新菜单。
                    if let Err(error) = refresh_tray_backup_menu(app) {
                        log::error!("failed to resync tray backup menu: {error}");
                    }
                }
            }
            TRAY_MENU_ID_QUIT => {
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
    repository_lock::record_process_start();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(
            // 刻意不调用 `.level()`：插件用 fern 的 dispatch 级别做静态过滤，
            // 一旦在此固定，运行时切换诊断模式（`log::set_max_level`）就无法让
            // 更低的等级通过。等级完全由 `app::diagnostics::apply_level` 控制，
            // 见 `src-tauri/src/app/diagnostics.rs` 的模块说明。
            //
            // 自定义格式在默认的「时间 + 等级 + target」之前插入本次运行标识，
            // 使轮转文件可以按运行切片；target 与等级本身就是可筛选的索引。
            tauri_plugin_log::Builder::new()
                .format(|out, message, record| {
                    let now = tauri_plugin_log::TimezoneStrategy::UseLocal.get_now();
                    out.finish(format_args!(
                        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}[run:{}][{}][{}] {}",
                        now.year(),
                        now.month() as u8,
                        now.day(),
                        now.hour(),
                        now.minute(),
                        now.second(),
                        app::diagnostics::run_id(),
                        record.level(),
                        record.target(),
                        message,
                    ))
                })
                .max_file_size(LOG_MAX_FILE_SIZE)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(LOG_KEPT_FILES))
                .build(),
        )
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|application| {
            let started = std::time::Instant::now();
            // 插件构建时没有固定等级，这里在最早的时机把生效等级写入全局过滤器。
            let diagnostics_enabled = app::diagnostics::default_enabled();
            app::diagnostics::apply_level(diagnostics_enabled);
            log::info!(
                "Lilith Artworks starting: version={}, diagnostics={}, log_level={}",
                env!("CARGO_PKG_VERSION"),
                if diagnostics_enabled { "on" } else { "off" },
                app::diagnostics::effective_level(diagnostics_enabled)
                    .as_str()
                    .to_ascii_lowercase()
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
            let app_state = app::AppState::new(settings, settings_path, log_directory, warning);
            application.manage(app_state.clone());
            // 诊断模式下由原生端线程看护 WebView 主线程，冻结期间仍能留下证据。
            app::diagnostics::start_webview_watchdog(application.handle().clone());
            let event_sink = std::sync::Arc::new(app::command_runtime::TauriEventSink::new(
                application.handle().clone(),
            ));
            let backup_state = backup::BackupState::default();
            backup_state.start_scheduler(application.handle().clone(), event_sink.clone())?;
            application.manage(backup_state.clone());
            sync_window_idle(application.handle());
            application.manage(app::OperationCoordinator::new(
                app_state,
                backup_state,
                event_sink,
            ));
            // 整画布导出的会话表：单并发，不参与仓库锁，随进程存活。
            application.manage(pin_board::PinBoardCanvasExportState::default());
            // 图片传输（导入/导出图片）的取消信号：同样是进程级、单并发。
            application.manage(pin_board::PinBoardTransferCancel::default());
            let resource_dir = application.path().resource_dir()?;
            // 只认随包资源目录。曾经保留一条 `CARGO_MANIFEST_DIR` 的编译期回退路径，
            // 那条路径会把开发机目录写进发布产物，也让「模型是否被替换」的判断多一个
            // 无法预测的落点；开发与发布两种构建都由打包把模型放进 `resources/models`。
            let model_candidates = [
                resource_dir.join("resources").join("models"),
                resource_dir.join("models"),
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
            // 图标缺失或损坏不该拦住启动：退回二进制里编译期嵌入的默认图标，
            // 但要留下告警，否则界面会在无人察觉的情况下换一个图标。
            let runtime_icon = match load_runtime_icons(&icon_path, tray_target_size(application)) {
                Ok(icons) => Some(icons),
                Err(error) => {
                    log::warn!(
                        "应用图标加载失败，改用二进制内嵌的默认图标：path={}, error={error}",
                        icon_path.display()
                    );
                    None
                }
            };
            if let (Some(window), Some(icons)) = (
                application.get_webview_window("main"),
                runtime_icon.as_ref(),
            ) {
                let _ = window.set_icon(icons.window.clone());
            }
            build_tray(application, runtime_icon.map(|icons| icons.tray))?;
            // 上一个进程被强杀或断电时，系统临时目录里会留下拖放暂存会话目录；
            // 正常退出由 `RunEvent::Exit` 删除本进程自己的那一份。
            let drop_stage_sweep = authenticity::drop_stage::sweep_stale();
            if drop_stage_sweep.reclaimed > 0 || drop_stage_sweep.failed > 0 {
                log::info!(
                    "drop stage sweep finished: reclaimed={}, failed={}",
                    drop_stage_sweep.reclaimed,
                    drop_stage_sweep.failed
                );
            }
            log::info!(
                "Lilith Artworks started: elapsed_ms={}",
                started.elapsed().as_millis()
            );
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Resized(_) | WindowEvent::Focused(_) if window.label() == "main" => {
                sync_window_idle(window.app_handle());
            }
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
                    mark_main_window_hidden(window.app_handle());
                } else if state.begin_shutdown_handshake() {
                    log::info!("Lilith Artworks shutting down");
                    api.prevent_close();
                    // Hiding first also fires the webview visibilitychange
                    // handler, which saves open pin boards as a side effect.
                    let _ = window.hide();
                    mark_main_window_hidden(window.app_handle());
                    begin_webview_shutdown(window.app_handle());
                } else {
                    // A handshake is already running; just keep the window shut.
                    api.prevent_close();
                    let _ = window.hide();
                    mark_main_window_hidden(window.app_handle());
                }
            }
            _ => {}
        })
        .invoke_handler(app::command_registry::handler())
        .build(tauri::generate_context!())
        .expect("error while building Lilith Artworks")
        .run(|_application, event| {
            // 退出路径只有这里一处能同时覆盖确认退出与哨兵强退，拖放暂存目录的
            // 删除因此挂在这里，而不是挂在某一条具体的关闭流程上。
            if let tauri::RunEvent::Exit = event {
                authenticity::drop_stage::remove_session();
            }
        });
}
