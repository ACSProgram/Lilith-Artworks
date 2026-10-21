import { createContext, useContext } from "react";
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

export interface RuntimeProgressContextValue {
  runtime: BackupRuntimeStatus;
  cancelPending: boolean;
  refresh: () => Promise<BackupRuntimeStatus>;
  setPolling: (active: boolean) => void;
  cancel: () => Promise<boolean>;
}

/**
 * 运行态上下文的独立模块，理由同 `appRuntimeContext.ts`：与 Provider 同文件会在热更新时
 * 换掉 context 对象身份，让消费者 `useContext` 拿到 `null` 而抛错。
 */
export const RuntimeProgressContext = createContext<RuntimeProgressContextValue | null>(null);

export function useRuntimeProgress(): RuntimeProgressContextValue {
  const context = useContext(RuntimeProgressContext);
  if (!context) {
    throw new Error("useRuntimeProgress 必须在 RuntimeProgressProvider 内使用");
  }
  return context;
}
