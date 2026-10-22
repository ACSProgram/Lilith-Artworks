//! 主窗口几何：恢复、捕获与显示器可见性校验。
//!
//! 纯几何判断放在这里做单测；`capture_window_settings` 只在窗口处于正常状态时
//! 采信尺寸与位置，异常几何（任务栏图标矩形、铺满屏幕的最大化矩形）不会被写回
//! 设置。不负责设置文件格式或写盘顺序。

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize};

use super::model::{MAX_WINDOW_HEIGHT, MAX_WINDOW_WIDTH, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH};
use super::state::AppState;

/// 恢复窗口位置的最低门槛：窗口矩形同某块显示器的重叠不足这个范围时，视为落在
/// 已经不存在的显示器上，改为居中，避免窗口开到屏幕外让用户以为程序没启动。
const MIN_VISIBLE_WINDOW_WIDTH: i32 = 160;
const MIN_VISIBLE_WINDOW_HEIGHT: i32 = 80;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
