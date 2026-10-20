import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import { APP_EVENT_NAME, APP_EVENT_VERSION, type AppEvent } from "./appEvents";
import { diagnosticsLog, isTauriRuntime } from "./diagnostics";
import { errorMessage } from "./errors";

type RuntimeEventListener = (event: AppEvent) => void;

const listeners = new Set<RuntimeEventListener>();
let bridgeStarted = false;
let lastTimestamp = 0;
let unknownVersion = 0;
let activeRepositoryKey: string | null = null;

/**
 * 仓库 key 只用于相等比较：去掉 Windows 扩展前缀、统一分隔符与大小写，
 * 并去掉结尾分隔符，避免原生侧与设置侧一处带 `\`、一处不带时把事件全部丢掉。
 *
 * 同步要求：本函数是后端 `platform::path::display_path`（剥 `\\?\` 前缀）+
 * `platform::path::equivalence_key`（统一分隔符与大小写）再加「去掉结尾分隔符」
 * 的组合（`src-tauri/src/platform/path.rs`）。后端任一环节改语义，这里必须对齐，
 * 见 `docs/guides/platform.md` 的例外登记表。
 */
function normalizeRepositoryKey(value: string | null | undefined): string | null {
  if (!value) return null;
  const normalized = value
    .replace(/^\\\\\?\\/, "")
    .replaceAll("/", "\\")
    .replace(/\\+$/, "")
    .toLowerCase();
  return normalized || null;
}

/**
 * 设置当前仓库 key。事件 bridge 据此丢弃其它仓库的事件，避免仓库切换后
 * 旧进程/旧命令的事件覆盖新仓库状态。乱序时间戳一并归零：新仓库的事件
 * 时间戳可能小于上一个仓库，沿用旧值会把它们全部当成过期事件。
 */
export function setRuntimeEventRepositoryKey(repositoryKey: string | null): void {
  activeRepositoryKey = normalizeRepositoryKey(repositoryKey);
  lastTimestamp = 0;
}

function warnUnknownVersion(version: number): void {
  if (unknownVersion === version) return;
  unknownVersion = version;
  diagnosticsLog(
    "warn",
    `[runtime] ignoring app event with unsupported version=${version}, expected=${APP_EVENT_VERSION}`,
  );
}

/**
 * 事件 bridge 的唯一入口。原生监听与测试都从这里进入，保证版本、仓库过滤
 * 与乱序丢弃只有一份实现。较早的 `timestampMs` 视为过期事件直接丢弃。
 */
export function dispatchRuntimeEvent(event: AppEvent): void {
  if (!event || typeof event !== "object" || event.version !== APP_EVENT_VERSION) {
    warnUnknownVersion(event?.version ?? -1);
    return;
  }
  if (activeRepositoryKey) {
    const eventKey = normalizeRepositoryKey(event.repositoryKey);
    if (eventKey !== activeRepositoryKey) return;
  }
  if (typeof event.timestampMs === "number" && event.timestampMs > 0) {
    if (event.timestampMs < lastTimestamp) return;
    lastTimestamp = event.timestampMs;
  }
  for (const listener of [...listeners]) {
    try {
      listener(event);
    } catch (error) {
      diagnosticsLog(
        "warn",
        `[runtime] app event listener failed: type=${event.type}, error=${errorMessage(error)}`,
      );
    }
  }
}

function ensureBridge(): void {
  if (bridgeStarted || !isTauriRuntime()) return;
  bridgeStarted = true;
  void listen<AppEvent>(APP_EVENT_NAME, (event) => dispatchRuntimeEvent(event.payload))
    .catch((error) => {
      bridgeStarted = false;
      diagnosticsLog(
        "warn",
        `[runtime] app event subscription failed: ${errorMessage(error)}`,
      );
    });
}

function subscribeAppEvents(listener: RuntimeEventListener): () => void {
  listeners.add(listener);
  ensureBridge();
  return () => {
    listeners.delete(listener);
  };
}

export interface RuntimeSubscriptionOptions {
  /** 为 false 时不订阅（例如轮询未启用、模块未挂载）。 */
  enabled?: boolean;
  /** 只接收该仓库的事件；省略时依赖 bridge 的全局仓库过滤。 */
  repositoryKey?: string | null;
}

/**
 * 统一的 `AppEvent` v1 消费入口。所有 controller 共用同一个原生监听与同一套
 * 版本/仓库/乱序过滤，不各自 `listen`，也不重新定义事件。
 */
export function useRuntimeSubscription(
  listener: RuntimeEventListener,
  options: RuntimeSubscriptionOptions = {},
): void {
  const listenerRef = useRef(listener);
  listenerRef.current = listener;
  const enabled = options.enabled ?? true;
  const repositoryKey = normalizeRepositoryKey(options.repositoryKey ?? null);

  useEffect(() => {
    if (!enabled) return undefined;
    return subscribeAppEvents((event) => {
      if (repositoryKey && normalizeRepositoryKey(event.repositoryKey) !== repositoryKey) return;
      listenerRef.current(event);
    });
  }, [enabled, repositoryKey]);
}

/** 测试用：清空订阅、仓库过滤与乱序时间戳，不触碰原生监听。 */
export function resetRuntimeEventBridgeForTests(): void {
  listeners.clear();
  activeRepositoryKey = null;
  lastTimestamp = 0;
  unknownVersion = 0;
}
