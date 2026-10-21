import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { appApi } from "./api";
import {
  IDLE_BACKUP_RUNTIME,
  RuntimeProgressContext,
  type RuntimeProgressContextValue,
} from "./runtimeProgressContext";
import { useRuntimeSubscription } from "../shared/useRuntimeSubscription";

/** 事件丢失或后台调度任务（无 coordinator 事件）时的兜底快照间隔。 */
const RUNTIME_POLL_INTERVAL_MS = 3_000;
/** 多个进度事件合并成一次状态读取，避免每个事件都打一次 IPC。 */
const RUNTIME_EVENT_REFRESH_MS = 150;

/**
 * 共享运行态。设置页与维护操作只在需要时打开订阅（`setPolling`）：
 * `AppEvent` 负责把长任务进度即时送达，3 秒快照轮询作为事件丢失与后台
 * 调度任务的兜底。命令结果仍负责最终状态。
 */
export function RuntimeProgressProvider({ children }: { children: ReactNode }) {
  const [runtime, setRuntime] = useState(IDLE_BACKUP_RUNTIME);
  const [cancelPending, setCancelPending] = useState(false);
  const [polling, setPolling] = useState(false);
  const refreshTimer = useRef<number | null>(null);
  /** 只有报告过进度的操作才值得在完成事件上再读一次状态；普通读命令不发进度。 */
  const progressOperations = useRef(new Set<string>());

  const refresh = useCallback(async () => {
    const next = await appApi.getBackupRuntimeStatus();
    setRuntime(next);
    return next;
  }, []);

  const scheduleRefresh = useCallback(() => {
    if (refreshTimer.current !== null) return;
    refreshTimer.current = window.setTimeout(() => {
      refreshTimer.current = null;
      void refresh().catch(() => undefined);
    }, RUNTIME_EVENT_REFRESH_MS);
  }, [refresh]);

  useEffect(() => () => {
    if (refreshTimer.current !== null) window.clearTimeout(refreshTimer.current);
  }, []);

  useRuntimeSubscription((event) => {
    // 素材板写入不经过共享运行锁，进度事件不影响这里的运行状态。
    if (event.domain === "pinBoard") return;
    if (event.type === "operationProgress") {
      if (event.operationId) progressOperations.current.add(event.operationId);
      scheduleRefresh();
      return;
    }
    if (event.type === "operationFinished") {
      const tracked = event.operationId
        ? progressOperations.current.delete(event.operationId)
        : false;
      if (tracked) scheduleRefresh();
      return;
    }
    if (event.type === "cleanupQueueChanged") scheduleRefresh();
  }, { enabled: polling });

  useEffect(() => {
    if (!polling) return undefined;
    let disposed = false;
    const poll = async () => {
      try {
        const next = await appApi.getBackupRuntimeStatus();
        if (!disposed) setRuntime(next);
      } catch {
        // 轮询是尽力而为；命令结果仍会报告失败。
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), RUNTIME_POLL_INTERVAL_MS);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [polling]);

  const cancel = useCallback(async () => {
    setCancelPending(true);
    try {
      return await appApi.cancelBackupOperation();
    } finally {
      setCancelPending(false);
    }
  }, []);

  const value = useMemo<RuntimeProgressContextValue>(() => ({
    runtime,
    cancelPending,
    refresh,
    setPolling,
    cancel,
  }), [cancel, cancelPending, refresh, runtime]);

  return (
    <RuntimeProgressContext.Provider value={value}>
      {children}
    </RuntimeProgressContext.Provider>
  );
}
