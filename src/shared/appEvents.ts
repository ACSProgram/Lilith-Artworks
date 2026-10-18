/**
 * `AppEvent` v1 契约（0.3 阶段 1）。
 *
 * 信封字段与语义冻结：后续只能追加可选字段，新事件类型以独立版本号引入。
 * 前端由 `useRuntimeSubscription.ts` 的单一 bridge 消费这一个通道；
 * 命令响应仍负责最终状态。
 */
export const APP_EVENT_VERSION = 1;
export const APP_EVENT_NAME = "app_event";

export type OperationOutcome = "succeeded" | "cancelled" | "failed";

export interface CommandErrorSummary {
  code: string;
  kind: string;
  message: string;
  retryable: boolean;
}

interface AppEventBase {
  version: number;
  repositoryKey?: string;
  operationId?: string;
  domain?: string;
  phase?: string;
  current?: number;
  total?: number;
  cancellable: boolean;
  completionRevision: number;
  timestampMs: number;
}

export type AppEvent =
  | (AppEventBase & { type: "operationStarted" })
  | (AppEventBase & { type: "operationProgress" })
  | (AppEventBase & {
      type: "operationFinished";
      outcome: OperationOutcome;
      error?: CommandErrorSummary;
    })
  | (AppEventBase & { type: "repositoryChanged" })
  | (AppEventBase & { type: "cleanupQueueChanged"; pendingCount?: number });
