/**
 * 共享运行状态 DTO：应用层（`app/api.ts`）与历史模块（`history/api.ts`）读取同一个
 * `get_backup_runtime_status`，因此定义放在这里而不是任一方的 `types.ts`。
 */

/** 当前占用共享运行锁的任务类型；与后端 `BackupTaskKind` 的序列化值一致。 */
export type BackupTaskKind = "automaticBackup" | "idleVerify" | "userOperation";

export interface BackupRuntimeStatus {
  busy: boolean;
  activeBranchId: string | null;
  taskKind: BackupTaskKind | null;
  /**
   * 上报进度的运行域（后端 `OperationDomain` 的序列化值，如 `maintenance`、
   * `cleanup`），并行后台任务汇总时为 `background`；尚未上报进度时为 `null`。
   * 它不是命令名。
   */
  operation: string | null;
  progressLabel: string | null;
  progressCurrent: number;
  progressTotal: number;
  automaticScheduling: boolean;
  completionRevision: number;
}
