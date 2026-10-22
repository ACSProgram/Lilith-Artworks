import { useCallback, useRef, useState } from "react";
import { errorMessage } from "../../shared/errors";

export type AuthenticityOperationKind = "preview" | "publish";

const CANCELLED_MESSAGE: Record<AuthenticityOperationKind, string> = {
  preview: "质量预览已取消。",
  publish: "签名发布已取消。",
};

interface OperationState {
  active: AuthenticityOperationKind | null;
  cancelRequested: boolean;
  /** 取消请求已被原生端接受、任务仍在运行：由任务收尾报告取消。 */
  notice: AuthenticityOperationKind | null;
  /** 任务先于取消请求结束：由取消入口报告取消。 */
  completed: AuthenticityOperationKind | null;
}

const IDLE: OperationState = { active: null, cancelRequested: false, notice: null, completed: null };

interface AuthenticityOperationOptions {
  onError: (message: string) => void;
  cancelOperation: () => Promise<boolean>;
  /** 取消请求未被接受（任务已经结束）时复位控制器的忙碌状态。 */
  onCancelRejected: () => void;
}

export interface AuthenticityOperation {
  cancelling: boolean;
  isActive: () => boolean;
  isCancelRequested: () => boolean;
  /** 开始一次活动任务，清掉上一次的取消标记。 */
  begin: (kind: AuthenticityOperationKind) => void;
  /**
   * 结束活动任务。`cancelled` 表示任务确实以取消结果结束；取消提示由这里与
   * `cancel()` 中先到的一方负责，避免正常完成被误报为已取消。
   */
  finish: (kind: AuthenticityOperationKind, cancelled: boolean) => void;
  /** 只复位等待取消的指示，用于切换分支等会让当前任务失效的场景。 */
  resetCancelling: () => void;
  cancel: () => Promise<void>;
}

/**
 * 质量预览与签名发布共用的活动任务生命周期：哪个任务在跑、是否已请求取消、
 * 取消提示由谁报告，以及等待取消收尾时的 `cancelling`。
 */
export function useAuthenticityOperation({
  onError,
  cancelOperation,
  onCancelRejected,
}: AuthenticityOperationOptions): AuthenticityOperation {
  const [cancelling, setCancelling] = useState(false);
  const state = useRef<OperationState>({ ...IDLE });

  const begin = useCallback((kind: AuthenticityOperationKind) => {
    state.current = { active: kind, cancelRequested: false, notice: null, completed: null };
    setCancelling(false);
  }, []);

  const finish = useCallback((kind: AuthenticityOperationKind, cancelled: boolean) => {
    const reportNow = cancelled && state.current.notice === kind;
    state.current = {
      active: null,
      cancelRequested: false,
      notice: null,
      completed: cancelled && !reportNow ? kind : null,
    };
    setCancelling(false);
    if (reportNow) onError(CANCELLED_MESSAGE[kind]);
  }, [onError]);

  const cancel = useCallback(async () => {
    const kind = state.current.active;
    state.current.cancelRequested = true;
    setCancelling(true);
    try {
      const accepted = await cancelOperation();
      if (accepted && kind) {
        if (state.current.completed === kind) {
          state.current.completed = null;
          onError(CANCELLED_MESSAGE[kind]);
        } else if (state.current.active === kind) {
          state.current.notice = kind;
        }
      } else {
        state.current = { ...IDLE };
        setCancelling(false);
        onCancelRejected();
      }
    } catch (error) {
      state.current.cancelRequested = false;
      state.current.notice = null;
      state.current.completed = null;
      setCancelling(false);
      onError(errorMessage(error));
    }
  }, [cancelOperation, onCancelRejected, onError]);

  const isActive = useCallback(() => state.current.active !== null, []);
  const isCancelRequested = useCallback(() => state.current.cancelRequested, []);
  const resetCancelling = useCallback(() => setCancelling(false), []);

  return { cancelling, isActive, isCancelRequested, begin, finish, resetCancelling, cancel };
}
