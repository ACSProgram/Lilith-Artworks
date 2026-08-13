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
  lastCheckMs: number | null;
  lastSuccessMs: number | null;
  lastError: string | null;
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
  backupEnabled: boolean;
  backupIntervalMinutes: number;
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

export interface BackupRuntimeStatus {
  busy: boolean;
  activeBranchId: string | null;
  operation: string | null;
  progressLabel: string | null;
  progressCurrent: number;
  progressTotal: number;
  automaticScheduling: boolean;
}
