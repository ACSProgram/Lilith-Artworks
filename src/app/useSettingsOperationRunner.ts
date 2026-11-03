import { useCallback, useEffect, useState } from "react";
import { IDLE_BACKUP_RUNTIME, useRuntimeProgress } from "./runtimeProgressContext";
import type { BackupRuntimeStatus } from "../shared/backupRuntime";
import { errorMessage } from "../shared/errors";

export type SettingsOperation =
  | "repository-scrub"
  | "repository-backup"
  | "unreferenced-scan"
  | "storage-scan";

export type Notify = (message: string | null) => void;

/**
 * 设置页长任务在后端上报的运行域。
 *
 * 运行状态的 `operation` 是 `OperationDomain` 的序列化值而不是命令名：完整性扫描、
 * 整仓备份与磁盘扫描都属 `maintenance`，垃圾文件扫描属 `cleanup`。据此只认设置页
 * 自己的任务，发布（`authenticity`）、素材板（`pinBoard`）等前台任务与后台调度
 * 任务（`taskKind` 不是 `userOperation`、`operation` 为 `background`）都不会把
 * 进度串到这个弹窗。
 */
const SETTINGS_OPERATION_DOMAINS = new Set(["maintenance", "cleanup"]);

/** 设置页长任务互斥提示：四个入口共用同一份文案，避免各写一遍。 */
const SETTINGS_BUSY_MESSAGE = "已有备份操作正在运行，请等待完成或先取消当前操作。";

/**
 * 四个设置页长任务共用的前置流程：读取运行状态 → busy 拒绝 → 占位 → 清提示 →
 * 执行 → 统一收尾。空间/备份/完整性三个域 hook 共用这一个闸门。
 */
export function useSettingsOperationRunner(settingsOpen: boolean, notify: Notify) {
  const {
    runtime,
    cancelPending,
    refresh: refreshRuntime,
    setPolling,
    cancel,
  } = useRuntimeProgress();
  const [settingsOperation, setSettingsOperation] = useState<SettingsOperation | null>(null);

  useEffect(() => {
    const active = settingsOpen || settingsOperation !== null;
    setPolling(active);
    return () => setPolling(false);
  }, [setPolling, settingsOpen, settingsOperation]);

  // 操作刚启动时立即读一次运行状态：事件会让进度随后持续到达，但首帧
  // 不能等 3 秒兜底轮询。
  useEffect(() => {
    if (settingsOperation !== null) void refreshRuntime().catch(() => undefined);
  }, [refreshRuntime, settingsOperation]);

  const runSettingsOperation = useCallback(async (
    operation: SettingsOperation,
    action: () => Promise<void>,
  ): Promise<void> => {
    let current: BackupRuntimeStatus;
    try {
      current = await refreshRuntime();
    } catch (error) {
      notify(errorMessage(error));
      return;
    }
    if (current.busy) {
      notify(SETTINGS_BUSY_MESSAGE);
      return;
    }
    setSettingsOperation(operation);
    notify(null);
    try {
      await action();
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setSettingsOperation(null);
    }
  }, [notify, refreshRuntime]);

  const cancelSettingsOperation = useCallback(async () => {
    try {
      const requested = await cancel();
      if (!requested) notify("操作尚未进入可取消阶段，请稍后重试。");
    } catch (error) {
      notify(errorMessage(error));
    }
  }, [cancel, notify]);

  const runtimeMatchesSettings = runtime.busy
    && runtime.taskKind === "userOperation"
    && runtime.operation !== null
    && SETTINGS_OPERATION_DOMAINS.has(runtime.operation);
  const visibleSettingsRuntime = runtimeMatchesSettings
    ? runtime
    : settingsOperation
      ? {
        ...IDLE_BACKUP_RUNTIME,
        busy: true,
        operation: settingsOperation,
        progressLabel: settingsOperation === "repository-backup"
          ? "正在准备创建备份"
          : settingsOperation === "unreferenced-scan"
            ? "正在准备垃圾文件扫描"
            : settingsOperation === "storage-scan"
              ? "正在准备磁盘占用扫描"
              : "正在准备完整性检查",
      }
      : null;

  return {
    runtime,
    cancelPending,
    settingsOperation,
    visibleSettingsRuntime,
    runSettingsOperation,
    cancelSettingsOperation,
  };
}
