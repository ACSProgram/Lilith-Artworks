export interface ArtworkHistory {
  artworkId: string;
  artworkTitle: string;
  branches: ArtworkBranch[];
  nodes: HistoryNode[];
}

export interface ArtworkBranch {
  id: string;
  title: string;
  sourcePath: string;
  headHistoryId: string | null;
  createdFromHistoryId: string | null;
  backupEnabled: boolean;
  backupIntervalMinutes: number;
  backupQuickEnabled: boolean;
  lastCheckMs: number | null;
  lastSuccessMs: number | null;
  lastError: string | null;
  consecutiveBackupFailures: number;
  backupRetryAtMs: number | null;
  backupDisableNoticePending: boolean;
  finalArtifactLocked: boolean;
  publishedCount: number;
  /** 最近一次空闲链路校验失败的摘要；与备份失败 `lastError` 相互独立。 */
  verifyError: string | null;
  /** 最近一次链路校验通过的时间；未校验过为 null。 */
  verifiedMs: number | null;
}

export interface HistoryNode {
  id: string;
  createdOnBranchId: string;
  parentId: string | null;
  title: string;
  note: string;
  commitKind: "manual" | "automatic";
  isCheckpoint: boolean;
  createdMs: number;
  logicalSize: number;
  chunkFileSize: number;
  sha256: string;
  chunkCount: number;
}

export interface ForkBranchRequest {
  artworkId: string;
  fromHistoryId: string;
  title: string;
  sourcePath: string;
}

export interface UpdateBranchBackupRequest {
  branchId: string;
  title: string;
  expectedBackupEnabled: boolean;
  backupEnabled: boolean;
  backupIntervalMinutes: number;
  backupQuickEnabled: boolean;
  sourcePath?: string;
}

export interface RenameHistoryNodeRequest {
  historyId: string;
  title: string;
}

export interface BackupCommitResult {
  created: boolean;
  unchanged: boolean;
  historyId: string | null;
}

/** 当前占用共享运行锁的任务类型；与后端 `BackupTaskKind` 的序列化值一致。 */
export type BackupTaskKind = "automaticBackup" | "idleVerify" | "userOperation";

export interface BackupRuntimeStatus {
  busy: boolean;
  activeBranchId: string | null;
  taskKind: BackupTaskKind | null;
  operation: string | null;
  progressLabel: string | null;
  progressCurrent: number;
  progressTotal: number;
  automaticScheduling: boolean;
  completionRevision: number;
}
