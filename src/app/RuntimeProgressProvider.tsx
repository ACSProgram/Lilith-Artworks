import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { appApi } from "./api";
import type { BackupRuntimeStatus } from "./types";

export const IDLE_BACKUP_RUNTIME: BackupRuntimeStatus = {
  busy: false,
  activeBranchId: null,
  taskKind: null,
  operation: null,
  progressLabel: null,
  progressCurrent: 0,
  progressTotal: 0,
  automaticScheduling: false,
  completionRevision: 0,
};

const RUNTIME_POLL_INTERVAL_MS = 350;

interface RuntimeProgressContextValue {
  runtime: BackupRuntimeStatus;
  cancelPending: boolean;
  refresh: () => Promise<BackupRuntimeStatus>;
  setPolling: (active: boolean) => void;
  cancel: () => Promise<boolean>;
}

const RuntimeProgressContext = createContext<RuntimeProgressContextValue | null>(null);

/**
 * 共享运行态轮询。设置页与维护操作只在需要时打开轮询（`setPolling`），
 * 命令结果仍负责最终状态；这里只提供进度展示与取消入口。
 */
export function RuntimeProgressProvider({ children }: { children: ReactNode }) {
  const [runtime, setRuntime] = useState(IDLE_BACKUP_RUNTIME);
  const [cancelPending, setCancelPending] = useState(false);
  const [polling, setPolling] = useState(false);

  const refresh = useCallback(async () => {
    const next = await appApi.getBackupRuntimeStatus();
    setRuntime(next);
    return next;
  }, []);

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

export function useRuntimeProgress(): RuntimeProgressContextValue {
  const context = useContext(RuntimeProgressContext);
  if (!context) {
    throw new Error("useRuntimeProgress 必须在 RuntimeProgressProvider 内使用");
  }
  return context;
}
