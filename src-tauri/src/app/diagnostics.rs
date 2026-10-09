//! 日志与诊断体系。
//!
//! 这里集中三件事：
//!
//! 1. **等级策略**：基线等级由 `LILITH_LOG_LEVEL` 指定；未指定时调试构建为
//!    Debug、发布构建为 Info。运行时可切换「诊断模式」（`set_max_level`），
//!    不需要重启，也不需要重新构建。
//! 2. **前端桥接**：WebView 侧没有写应用日志文件的路径，主线程卡死时也拿不到
//!    堆栈。`log_frontend_diagnostics` 把前端事件转发进同一份日志，使「前端最后
//!    一次活动时间」能与原生端心跳对齐。
//! 3. **状态查询与切换命令**：`get_diagnostics_status` /
//!    `set_diagnostics_enabled`，设置页据此提供开关。
//!
//! 等级开关为什么不在插件构建时设置：`tauri-plugin-log` 用 fern 的
//! `Dispatch::level` 做静态过滤，一旦在 builder 里固定，运行时再调
//! `log::set_max_level` 也无法让更低的等级通过。因此 `lib.rs` 构建插件时刻意
//! **不调用** `.level()`（dispatch 保持默认的 Trace），由 [`apply_level`] 成为
//! 唯一的等级开关。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::app::AppState;

/// 本次运行的短标识。写进每一行日志，便于从按大小轮转的文件里切出单次运行：
/// `grep '\[run:ab12cd\]'`。取值来自启动时刻低位与进程号的异或，不保证全局唯一，
/// 只要求同一次运行内稳定、不同次运行大概率不同。
static RUN_ID: OnceLock<String> = OnceLock::new();

/// 返回本次运行的短标识（6 位十六进制）。
pub(crate) fn run_id() -> &'static str {
    RUN_ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        format!(
            "{:06x}",
            (nanos ^ u64::from(std::process::id())) & 0x00ff_ffff
        )
    })
}

/// 常规等级：只记录操作级事件。发布构建的默认值，正常使用不受影响。
const NORMAL_LEVEL: log::LevelFilter = log::LevelFilter::Info;
/// 详细等级：附加逐步骤事件与框架内部事件，供复现与定位使用。
const DETAILED_LEVEL: log::LevelFilter = log::LevelFilter::Debug;

/// 看门狗 ping 间隔。诊断模式下每 2 秒向前端发一次 ping，前端立即回 pong。
const WATCHDOG_INTERVAL_MS: u64 = 2_000;
/// 超过该时长没有 pong 即判定 WebView 主线程卡死。
const WATCHDOG_STALL_MS: u64 = 6_000;
/// 启动宽限：WebView 加载并注册监听需要时间，此前不报告卡死。
const WATCHDOG_GRACE_MS: u64 = 15_000;

/// `LILITH_LOG_LEVEL` 指定的基线等级；未设置或无法识别时为 `None`。
pub(crate) fn baseline_level_from_env() -> Option<log::LevelFilter> {
    let value = std::env::var("LILITH_LOG_LEVEL").ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Some(log::LevelFilter::Off),
        "error" => Some(log::LevelFilter::Error),
        "warn" => Some(log::LevelFilter::Warn),
        "info" => Some(log::LevelFilter::Info),
        "debug" => Some(log::LevelFilter::Debug),
        "trace" => Some(log::LevelFilter::Trace),
        _ => None,
    }
}

/// 基线等级：环境变量优先，否则调试构建 Debug、发布构建 Info。
pub(crate) fn baseline_level() -> log::LevelFilter {
    baseline_level_from_env().unwrap_or(if cfg!(debug_assertions) {
        DETAILED_LEVEL
    } else {
        NORMAL_LEVEL
    })
}

/// 诊断模式默认是否开启。基线达到 Debug 即视为开启，因此调试构建默认开启、
/// 发布构建默认关闭。
pub(crate) fn default_enabled() -> bool {
    baseline_level() >= DETAILED_LEVEL
}

/// 生效等级：诊断模式开启时至少为 [`DETAILED_LEVEL`]，否则取基线。
///
/// 环境变量把基线抬到 debug/trace 时，关闭诊断模式也不会降级，避免「关掉开关
/// 反而丢掉本来该有的详细日志」；反之，把基线降到 warn/error/off 也能真正生效。
pub(crate) fn effective_level(enabled: bool) -> log::LevelFilter {
    let baseline = baseline_level();
    if enabled {
        std::cmp::max(baseline, DETAILED_LEVEL)
    } else {
        baseline
    }
}

/// 把生效等级写入全局过滤器。插件未设置 dispatch 级别，因此这里是唯一的等级开关。
pub(crate) fn apply_level(enabled: bool) {
    log::set_max_level(effective_level(enabled));
}

/// 诊断模式状态，随应用生命周期存活，不写入设置文件。
#[derive(Debug)]
pub(crate) struct DiagnosticsState {
    enabled: AtomicBool,
    /// 看门狗时钟起点；`last_pong_ms` 与 `elapsed_ms` 共用它，保证可比。
    started: Instant,
    /// 最近一次收到前端 pong 的毫秒时间戳（相对 `started`）。
    last_pong_ms: AtomicU64,
}

impl DiagnosticsState {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            started: Instant::now(),
            last_pong_ms: AtomicU64::new(0),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub(crate) fn effective_level(&self) -> log::LevelFilter {
        effective_level(self.enabled())
    }

    /// 切换诊断模式并立即更新全局过滤器。
    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        apply_level(enabled);
    }

    /// 相对看门狗起点的毫秒数，作为 ping/pong 的共同时钟。
    pub(crate) fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub(crate) fn last_pong_ms(&self) -> u64 {
        self.last_pong_ms.load(Ordering::SeqCst)
    }

    /// 记录一次前端 pong。
    pub(crate) fn record_pong(&self) {
        self.last_pong_ms.store(self.elapsed_ms(), Ordering::SeqCst);
    }

    /// 把 pong 基准推进到当前时刻。诊断模式关闭或刚开启时调用，避免误报卡死。
    pub(crate) fn reset_pong_baseline(&self) {
        self.record_pong();
    }
}

/// 看门狗：诊断模式下每 2 秒向前端发 ping；前端 pong 停摆即判定主线程卡死。
///
/// 这是唯一能在「冻结进行中」留下证据的位置——前端此时已经写不出任何日志，
/// 而原生端线程仍在运行，且该记录能扛过随后的 15 秒兜底强退。
pub(crate) fn start_webview_watchdog(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        use tauri::{Emitter, Manager};

        let mut sequence: u64 = 0;
        let mut observed_pong_ms: u64 = 0;
        let mut stall_since_ms: Option<u64> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(WATCHDOG_INTERVAL_MS));
            let state = app.state::<AppState>();
            let diagnostics = state.diagnostics();
            if !diagnostics.enabled() {
                // 关闭诊断模式时不打扰 WebView，也不报告卡死。
                diagnostics.reset_pong_baseline();
                observed_pong_ms = diagnostics.last_pong_ms();
                stall_since_ms = None;
                continue;
            }
            sequence += 1;
            let _ = app.emit("diagnostics_ping", sequence);
            let now_ms = diagnostics.elapsed_ms();
            let pong_ms = diagnostics.last_pong_ms();
            if pong_ms > observed_pong_ms {
                observed_pong_ms = pong_ms;
                if let Some(from_ms) = stall_since_ms.take() {
                    log::warn!(
                        "[watchdog] webview responded again: main thread was unresponsive for {} ms",
                        pong_ms.saturating_sub(from_ms)
                    );
                }
            } else if stall_since_ms.is_none()
                && now_ms >= WATCHDOG_GRACE_MS
                && now_ms.saturating_sub(pong_ms) >= WATCHDOG_STALL_MS
            {
                log::warn!(
                    "[watchdog] webview unresponsive: no pong for {} ms (last pong at {} ms, ping #{sequence}); main thread is likely blocked",
                    now_ms.saturating_sub(pong_ms),
                    pong_ms
                );
                stall_since_ms = Some(pong_ms);
            }
        }
    });
}

#[tauri::command]
pub(crate) fn diagnostics_pong(state: tauri::State<'_, AppState>) {
    state.diagnostics().record_pong();
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticsStatus {
    enabled: bool,
    /// 当前生效等级的小写名称（off/error/warn/info/debug/trace）。
    level: String,
    log_directory: String,
}

fn status(state: &AppState) -> DiagnosticsStatus {
    let diagnostics = state.diagnostics();
    DiagnosticsStatus {
        enabled: diagnostics.enabled(),
        level: diagnostics.effective_level().as_str().to_ascii_lowercase(),
        log_directory: state.log_directory().to_string_lossy().into_owned(),
    }
}

#[tauri::command]
pub(crate) fn get_diagnostics_status(state: tauri::State<'_, AppState>) -> DiagnosticsStatus {
    status(&state)
}

/// 切换诊断模式。切换结果通过 `diagnostics_mode_changed` 事件广播，前端据此即时
/// 启停主线程心跳，无需重启。
#[tauri::command]
pub(crate) fn set_diagnostics_enabled(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    enabled: bool,
) -> DiagnosticsStatus {
    use tauri::Emitter;

    let previous = state.diagnostics().enabled();
    state.diagnostics().set_enabled(enabled);
    let next = status(&state);
    if previous != enabled {
        log::info!(
            "diagnostics mode {}: level={}",
            if enabled { "enabled" } else { "disabled" },
            next.level
        );
    }
    let _ = app.emit("diagnostics_mode_changed", &next);
    next
}

/// 前端诊断事件的等级。刻意不提供 debug/trace：前端转发的事件都应是需要留痕的
/// 操作级或异常级信息。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FrontendLevel {
    Info,
    Warn,
    Error,
}

/// 记录一条前端诊断事件。
///
/// 前端只转发事件驱动的低频记录（生命周期、保存、结算、阈值告警），不再有逐帧或周期性
/// 来源，因此三个等级都直接写入；周期性探针只在诊断档于前端侧启停。
#[tauri::command]
pub(crate) fn log_frontend_diagnostics(level: FrontendLevel, message: String) {
    // 统一用 `webview` target，便于按来源筛选前端日志；等级与 tag 保留在消息里。
    match level {
        FrontendLevel::Info => log::info!(target: "webview", "{message}"),
        FrontendLevel::Warn => log::warn!(target: "webview", "{message}"),
        FrontendLevel::Error => log::error!(target: "webview", "{message}"),
    }
}
