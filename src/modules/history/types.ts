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
