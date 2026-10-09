/**
 * 前端诊断层。分两档：
 *
 * - **常规档**（默认）：事件驱动的低频记录——生命周期、保存、结算、切换、GPU 设备健康，
 *   以及阈值触发的慢步骤与长任务。这些记录平时零噪声、开销可忽略，因此常开，用于捕捉
 *   偶发事件。
 * - **诊断档**（设置页「调试」栏开关）：追加周期性探针（2 秒心跳、500 ms 事件循环延迟）
 *   与取证通道（面包屑、独立线程 Worker）。探针只在异常时写日志，因此噪声很低，但仍有
 *   持续开销，只在排查问题时启用。
 *
 * 全局 `error` / `unhandledrejection` 捕获始终生效，与档位无关。
 */
import { listen } from "@tauri-apps/api/event";
import { invokeCommand } from "./tauri";
import type { ForensicsEvent } from "./diagnosticsProtocol";

/** 前端可写入应用日志的等级；debug/trace 由原生端内部使用，前端不转发。 */
export type DiagnosticsLevel = "info" | "warn" | "error";

export interface DiagnosticsStatus {
  enabled: boolean;
  /** 当前生效等级的小写名称（off/error/warn/info/debug/trace）。 */
  level: string;
  logDirectory: string;
}

/** 主线程心跳间隔。心跳只刷新取证通道与检测迟到，正常情况下不写日志。 */
const HEARTBEAT_INTERVAL_MS = 2_000;
/** 事件循环延迟探针：间隔与告警阈值，用于捕捉心跳粒度以下的主线程阻塞。 */
const LAG_PROBE_INTERVAL_MS = 500;
const LAG_WARN_MS = 800;
/** 长任务告警阈值：单次占用主线程超过该时长即记录。 */
const LONG_TASK_WARN_MS = 200;
/** 心跳里附带的「最近操作」最大长度，避免日志行过长。 */
const LAST_OPERATION_MAX = 120;

let sequence = 0;
let enabled = false;
let heartbeat: number | null = null;
let lagProbe: number | null = null;
let longTaskObserver: PerformanceObserver | null = null;
/** 最近一次记录的操作；上报给取证 Worker，使冻结记录自带上下文。 */
let lastOperation = "";
/**
 * 会话代次。React 严格模式下 effect 会挂载两次，先发起的会话可能后返回；代次
 * 保护让过期会话只清理自己的监听，不误停当前会话的探针。
 */
let session = 0;

/** 当前是否运行在 Tauri 宿主内；浏览器预览与测试环境为 false。 */
export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** 面包屑写入的最小间隔，避免同步存储写入拖慢主线程。 */
const BREADCRUMB_INTERVAL_MS = 1_000;
/** 面包屑在 localStorage 中的键名。 */
const BREADCRUMB_KEY = "lilith.diagnostics.breadcrumb";
/** 上一次写入面包屑的时间（performance.now）。 */
let breadcrumbAt = -Infinity;
/** 上一次会话的面包屑是否已上报（严格模式下会话会挂载两次）。 */
let breadcrumbReported = false;

/**
 * 把「最后活动」同步写入 localStorage。
 *
 * 这是唯一**不经过 IPC** 的取证通道。卡死有两种可能：主线程冻结（面包屑停在冻结前），
 * 或主线程活着而 IPC 停摆（心跳仍会持续刷新面包屑）。下次启动读回即可区分二者——
 * 仅靠日志做不到，因为日志本身要经过同一条 IPC。
 */
function writeBreadcrumb(): void {
  const now = performance.now();
  if (now - breadcrumbAt < BREADCRUMB_INTERVAL_MS) return;
  breadcrumbAt = now;
  try {
    window.localStorage.setItem(
      BREADCRUMB_KEY,
      `${new Date().toLocaleTimeString()} | ${lastOperation.slice(0, LAST_OPERATION_MAX)}`,
    );
  } catch {
    // 存储不可用（隐私模式等）时忽略；诊断失败不影响业务路径。
  }
}

/** 启动时报告上一次会话的最后活动时间与操作（来自 localStorage 面包屑）。 */
export function reportPreviousBreadcrumb(): void {
  if (!isTauriRuntime()) return;
  try {
    const previous = window.localStorage.getItem(BREADCRUMB_KEY);
    if (previous) {
      diagnosticsLog("warn", `[forensic] previous session last activity: ${previous}`);
    }
  } catch {
    // 忽略
  }
}

/**
 * 取证 Worker：独立线程，主线程冻结时仍能把「最后心跳 / 最后操作 / 静默时长」
 * 写进 IndexedDB。这是唯一一条不随主线程一起死掉的取证通道，也是定位冻结的
 * 关键——其余探针（心跳、看门狗、长任务）都只能证明「主线程停了」，证明不了
 * 「停在哪一步、停了多久、有没有恢复」。
 */
let worker: Worker | null = null;
/** 上一次向 Worker 上报心跳的时间（performance.now），用于节流。 */
let workerBeatAt = -Infinity;
/** 心跳上报的最小间隔：逐条日志都发消息会拖慢高频路径。 */
const WORKER_BEAT_INTERVAL_MS = 250;

function startWorker(): void {
  if (!isTauriRuntime() || typeof Worker === "undefined") return;
  if (worker === null) {
    try {
      worker = new Worker(new URL("./diagnosticsWorker.ts", import.meta.url), { type: "module" });
      worker.onmessage = (event: MessageEvent) => {
        const payload = event.data as ForensicsEvent | null;
        if (!payload || typeof payload !== "object") return;
        if (payload.type === "previous-freeze") {
          const record = payload.record;
          const recovered = record.recoveredAt
            ? `recovered after ${record.recoveredAt - record.at}ms`
            : "no recovery before exit";
          diagnosticsLog(
            "warn",
            `[forensic] previous freeze: silent ${record.silenceMs}ms, last beat #${record.lastBeatSeq} at ${new Date(record.lastBeatAt).toLocaleTimeString()}, last op="${record.lastOp}", ${recovered}`,
          );
        } else if (payload.type === "frozen") {
          // 本行只有在主线程恢复后才能写出；它出现即说明是长停顿而非永久挂起。
          diagnosticsLog(
            "warn",
            `[forensic] worker watchdog: main thread silent for ${payload.silenceMs}ms, last op="${payload.lastOp ?? ""}"`,
          );
        } else if (payload.type === "recovered") {
          diagnosticsLog(
            "warn",
            `[forensic] worker watchdog: main thread recovered after ~${payload.silenceMs}ms`,
          );
        }
      };
    } catch {
      worker = null;
      return;
    }
  }
  worker.postMessage({ type: "start" });
}

function stopWorker(): void {
  worker?.postMessage({ type: "stop" });
}

/** 把当前「最近操作」节流上报给取证 Worker。 */
function beatWorker(): void {
  // 诊断模式关闭时 Worker 处于 disarm 状态，继续上报只是浪费消息。
  if (worker === null || !enabled) return;
  const now = performance.now();
  if (now - workerBeatAt < WORKER_BEAT_INTERVAL_MS) return;
  workerBeatAt = now;
  try {
    worker.postMessage({
      type: "beat",
      at: Date.now(),
      seq: sequence,
      op: lastOperation.slice(0, LAST_OPERATION_MAX),
    });
  } catch {
    // Worker 已终止时忽略；取证失败不影响业务路径。
  }
}

/** 推进取证通道：刷新「最后活动」面包屑，并把最近操作节流上报给取证 Worker。 */
function pumpForensics(): void {
  writeBreadcrumb();
  beatWorker();
}

/** 发送一条诊断事件，不更新「最近操作」。 */
function send(level: DiagnosticsLevel, message: string): void {
  if (!isTauriRuntime()) return;
  // 常规档也转发 info：剩余的前端记录都是事件驱动的低频事件（生命周期、保存、结算、
  // 切换、阈值告警），已无逐帧或周期性来源，因此无需按档位过滤。
  // 取证通道只在诊断档启用：常规档不产生额外的存储写入与 Worker 消息开销。
  if (enabled) pumpForensics();
  const entry = `#${++sequence} ${message}`;
  try {
    void invokeCommand<void>("log_frontend_diagnostics", { level, message: entry })
      .catch(() => undefined);
  } catch {
    // 宿主不可用时 invoke 可能同步抛错；诊断失败不能影响业务路径。
  }
}

/**
 * 把一条前端事件写进应用日志，并记为「最近操作」。
 *
 * WebView 侧没有写日志文件的路径，卡死时也拿不到堆栈，只能通过原生端转发。
 * 三个等级都直接发送：前端只转发事件驱动的低频记录，周期性与逐帧内容已全部移出。
 */
export function diagnosticsLog(level: DiagnosticsLevel, message: string): void {
  lastOperation = message;
  send(level, message);
}

/**
 * 只在步骤耗时超过阈值时记录一条告警。用于高频路径（逐张纹理加载）——
 * 常速时不产生任何日志，因此常开，用于捕捉偶发变慢；一旦某步变慢即点名。
 */
export function diagnosticsSlowStep(name: string, startedAt: number, thresholdMs = 150): void {
  if (!isTauriRuntime()) return;
  const elapsed = performance.now() - startedAt;
  if (elapsed > thresholdMs) {
    diagnosticsLog("warn", `[slow] slow step: ${name} took ${Math.round(elapsed)}ms`);
  }
}

function startHeartbeat(): void {
  if (heartbeat !== null) return;
  let last = performance.now();
  heartbeat = window.setInterval(() => {
    const now = performance.now();
    const gap = Math.round(now - last);
    last = now;
    const drift = gap - HEARTBEAT_INTERVAL_MS;
    // 心跳本身保持静默，只把「最近存活」推进取证通道；仅当明显迟到时才记一条告警。
    // 冻结时刻由取证通道回读给出，无需每 2 秒刷一行日志。
    pumpForensics();
    if (drift > HEARTBEAT_INTERVAL_MS) {
      send(
        "warn",
        `[hb] main-thread heartbeat late gap=${gap}ms drift=${drift}ms last="${lastOperation.slice(0, LAST_OPERATION_MAX)}"`,
      );
    }
  }, HEARTBEAT_INTERVAL_MS);
}

function stopHeartbeat(): void {
  if (heartbeat === null) return;
  window.clearInterval(heartbeat);
  heartbeat = null;
}

/**
 * 事件循环延迟探针：回调迟到即说明主线程被同步阻塞，恢复后记录阻塞时长。
 * 用于捕捉心跳粒度以下、以及冻结前的逐步恶化。
 */
function startLagProbe(): void {
  if (lagProbe !== null) return;
  let expected = performance.now() + LAG_PROBE_INTERVAL_MS;
  lagProbe = window.setInterval(() => {
    const now = performance.now();
    const late = Math.round(now - expected);
    expected = now + LAG_PROBE_INTERVAL_MS;
    if (late > LAG_WARN_MS) {
      send("warn", `[lag] main-thread blocked for ~${late}ms (event-loop probe late)`);
    }
  }, LAG_PROBE_INTERVAL_MS);
}

function stopLagProbe(): void {
  if (lagProbe === null) return;
  window.clearInterval(lagProbe);
  lagProbe = null;
}

/**
 * 长任务观察器：主线程单次占用超过阈值时（在恢复后）记一条。
 * 用于捕捉「逐步恶化」——若冻结前出现一串越来越长的长任务，说明是渐进式阻塞
 * 而非某一步突然死锁。冻结本身不会触发（观察器回调也在主线程上）。
 */
function startLongTaskObserver(): void {
  if (longTaskObserver !== null) return;
  if (typeof PerformanceObserver === "undefined") return;
  try {
    longTaskObserver = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        if (entry.duration >= LONG_TASK_WARN_MS) {
          send(
            "warn",
            `[task] long task ${Math.round(entry.duration)}ms ending at ${Math.round(entry.startTime + entry.duration)}ms`,
          );
        }
      }
    });
    longTaskObserver.observe({ entryTypes: ["longtask"] });
  } catch {
    // 宿主不支持 longtask 时静默降级，不影响其余诊断。
    longTaskObserver = null;
  }
}

function stopLongTaskObserver(): void {
  if (longTaskObserver === null) return;
  longTaskObserver.disconnect();
  longTaskObserver = null;
}

function syncActivity(next: boolean): void {
  enabled = next;
  if (next) {
    startHeartbeat();
    startLagProbe();
    startWorker();
  } else {
    stopHeartbeat();
    stopLagProbe();
    stopWorker();
  }
}

function stopActivity(): void {
  enabled = false;
  stopHeartbeat();
  stopLagProbe();
  stopWorker();
}

/** 全局错误捕获是否已安装（严格模式下 `startDiagnostics` 会被调用两次）。 */
let globalCaptureInstalled = false;

/**
 * 接管 `window.onerror` 与 `unhandledrejection`。
 *
 * 这两类异常此前**完全不可见**：WebView2 不会把它们写进应用日志，而冻结前的最后一次
 * 未捕获异常往往就是原因本身。`error` 事件对脚本异常是 `ErrorEvent`、对资源加载失败
 * 是普通 `Event`，二者分开记录，避免把缺图当成脚本异常。
 */
function installGlobalErrorCapture(): void {
  if (globalCaptureInstalled) return;
  globalCaptureInstalled = true;
  // 显式标注为 `Event`：`addEventListener("error")` 的默认重载会把参数窄化成
  // `ErrorEvent`，那样就分辨不出资源加载失败（普通 `Event`）这一支。
  window.addEventListener("error", (event: Event) => {
    if (event instanceof ErrorEvent) {
      diagnosticsLog(
        "error",
        `[error] window error: ${event.message} @ ${event.filename ?? "?"}:${event.lineno}:${event.colno}${event.error ? ` | ${String(event.error)}` : ""}`,
      );
      return;
    }
    const target = event.target as Element | null;
    diagnosticsLog(
      "error",
      `[error] resource load error: ${target?.tagName ?? "unknown"} ${(target as HTMLImageElement | null)?.src ?? ""}`,
    );
  });
  window.addEventListener("unhandledrejection", (event) => {
    diagnosticsLog("error", `[error] unhandled rejection: ${String(event.reason)}`);
  });
}

/**
 * 接入诊断体系：应答原生端看门狗的 ping，读取当前状态决定是否启动心跳，并订阅
 * 开关变化，使设置页的切换无需重启即可生效。返回清理函数。
 *
 * ping 应答必须最先注册：原生端在冻结期间靠它判断主线程是否还活着，晚注册会
 * 被误判为卡死。
 */
export async function startDiagnostics(): Promise<() => void> {
  if (!isTauriRuntime()) return () => undefined;
  // 错误捕获与诊断模式无关：即便「详细日志」关闭，前端异常也应始终留痕。
  installGlobalErrorCapture();
  const id = ++session;
  const disposers: Array<() => void> = [];
  try {
    disposers.push(await listen<number>("diagnostics_ping", () => {
      if (id !== session) return;
      try {
        void invokeCommand<void>("diagnostics_pong").catch(() => undefined);
      } catch {
        // 宿主不可用时忽略；看门狗会据缺失的 pong 报告卡死。
      }
    }));
    const status = await invokeCommand<DiagnosticsStatus>("get_diagnostics_status");
    if (id === session) {
      // 长任务观察器常驻：被动监听，仅在单次占用主线程超过阈值时记一条，不受档位影响。
      startLongTaskObserver();
      syncActivity(status.enabled);
      if (!breadcrumbReported) {
        breadcrumbReported = true;
        reportPreviousBreadcrumb();
      }
      diagnosticsLog(
        "info",
        `diagnostics session started: enabled=${status.enabled}, level=${status.level}, logDir=${status.logDirectory}`,
      );
    }
    disposers.push(await listen<DiagnosticsStatus>("diagnostics_mode_changed", (event) => {
      if (id === session) syncActivity(event.payload.enabled);
    }));
    return () => {
      // 过期会话仍需移除自己的监听，但不能停掉当前会话的探针。
      for (const dispose of disposers) dispose();
      if (id === session) {
        stopActivity();
        stopLongTaskObserver();
      }
    };
  } catch {
    for (const dispose of disposers) dispose();
    // 诊断通道不可用时保持静默，不影响应用其余功能。
    return () => undefined;
  }
}
