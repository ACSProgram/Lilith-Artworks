import type { PinBoardTextureCacheLevel } from "../modules/pin-board/types";

export type Theme = "system" | "light" | "dark";
export type ContentDensity = "comfortable" | "compact";
export type BackupCheckMode = "quick" | "full";

export interface PinBoardSettings {
  textureCacheLevel: PinBoardTextureCacheLevel;
  arrangementGapRatio: number;
  autosave: boolean;
  saveOnExit: boolean;
  lockShortcut: string;
  fullscreenShortcut: string;
}

export interface WindowSettings {
  x: number | null;
  y: number | null;
  width: number;
  height: number;
  maximized: boolean;
}

export interface ContentSettings {
  density: ContentDensity;
}

export interface AppSettings {
  version: number;
  /** 设置文件自己的单调计数；保存时作为 CAS 的期望值。 */
  revision: number;
  repositoryPath: string;
  theme: Theme;
  closeToTray: boolean;
  pauseAutomaticBackups: boolean;
  /** 空闲完整性检查；默认关闭，仅窗口隐藏或最小化时逐批运行。 */
  idleIntegrityCheckEnabled: boolean;
  automaticBackupCheckMode: BackupCheckMode;
  /** 新节点是否使用 ChunkFile v2；旧 v1 节点始终可读。 */
  chunkFileFormatV2: boolean;
  window: WindowSettings;
  content: ContentSettings;
  pinBoard: PinBoardSettings;
}

export interface SettingsSnapshot {
  settings: AppSettings;
  settingsPath: string;
  logDirectory: string;
  warning: string | null;
  automaticBackupFileCount: number | null;
  /** 与空闲完整性检查同口径的待校验历史节点数。 */
  idleIntegrityPendingCount: number | null;
}

export interface RepositoryStatus {
  configured: boolean;
  ready: boolean;
  rootPath: string;
  databasePath: string;
  error: string | null;
}

/** 保存新仓库路径前的只读探测结果；`repository` 也包含可迁移的旧文件名数据库。 */
export type RepositoryPathState =
  | "missing"
  | "not_directory"
  | "empty_directory"
  | "repository"
  | "non_repository_directory";

/** 仓库完整性检查与整仓备份报告共有的计数。 */
export interface RepositoryIntegrityCounts {
  historyNodes: number;
  finalArtifacts: number;
  certificationRecords: number;
}

export interface RepositoryScrubReport extends RepositoryIntegrityCounts {
  /** 检查的画板位图记录数；文字条目没有实体文件，不计入。 */
  pinBoardBitmaps: number;
  /** 记录存在但 DDS 缺失。 */
  pinBoardMissingDds: number;
  /** DDS 存在但校验失败（路径归属、头、尺寸、长度或解码）。 */
  pinBoardCorruptDds: number;
  /** 磁盘上存在但无记录的孤儿 DDS。 */
  pinBoardOrphanDds: number;
}

export interface RepositoryBackupReport extends RepositoryIntegrityCounts {
  backupPath: string;
  repositoryPath: string;
  fileCount: number;
  totalBytes: number;
  /** 本次备份启动前回收的残留灾备暂存目录数量。 */
  reclaimedStagingDirectories: number;
  /** 未能回收的残留暂存目录数量；不为零也不阻断本次备份。 */
  failedStagingDirectories: number;
}

/** 持久化的最近一次整仓备份结果；重启后由设置页读取。 */
export interface RepositoryBackupRun extends RepositoryIntegrityCounts {
  backupPath: string;
  repositoryPath: string;
  fileCount: number;
  totalBytes: number;
  createdMs: number;
}

export interface BackupDisableNoticeTarget {
  artworkId: string;
  branchId: string;
}

export type StorageCategoryKind =
  | "snapshot"
  | "delta"
  | "finalArtifact"
  | "certification"
  | "boardDds"
  | "other";

export interface StorageRatio {
  logicalBytes: number;
  storedBytes: number;
  ratio: number | null;
}

export interface StorageCategory {
  kind: StorageCategoryKind;
  bytes: number;
  fileCount: number;
}

export interface StorageCompression {
  snapshot: StorageRatio;
  delta: StorageRatio;
}

export interface StorageNode {
  id: string;
  title: string;
  createdMs: number;
  isCheckpoint: boolean;
  hasSnapshot: boolean;
  logicalSize: number;
  /** snapshot 行是快照文件大小；其余行是重建该节点需要读取的字节数。 */
  chunkFileSize: number;
  chunkCount: number;
  compressionRatio: number | null;
  snapshotPath: string | null;
  deltaPath: string | null;
}

export interface StorageArtifact {
  id: string;
  sourcePath: string;
  byteSize: number;
  createdMs: number;
}

export interface StorageCertification {
  id: string;
  title: string;
  outputPath: string;
  storedPath: string;
  outputBytes: number;
  createdMs: number;
}

export interface StorageBranch {
  branchId: string;
  title: string;
  snapshotBytes: number;
  deltaBytes: number;
  finalArtifactBytes: number;
  certificationBytes: number;
  recordedBytes: number;
  finalArtifact: StorageArtifact | null;
  certificationRecords: StorageCertification[];
  nodes: StorageNode[];
}

export interface StorageBoard {
  boardId: number;
  name: string;
  trashed: boolean;
  imageCount: number;
}

export interface StorageArtwork {
  artworkId: string;
  title: string;
  groupPath: string[];
  trashed: boolean;
  snapshotBytes: number;
  deltaBytes: number;
  finalArtifactBytes: number;
  certificationBytes: number;
  recordedBytes: number;
  nodeCount: number;
  branchCount: number;
  branches: StorageBranch[];
  boards: StorageBoard[];
}

export interface StorageUnassigned {
  kind: StorageCategoryKind;
  bytes: number;
  count: number;
}

export interface RepositoryStorageOverview {
  calculatedAtMs: number;
  databaseRecordedBytes: number;
  categories: StorageCategory[];
  compression: StorageCompression;
  artworks: StorageArtwork[];
  unassigned: StorageUnassigned[];
}

export interface StorageOtherBreakdown {
  sqliteBytes: number;
  tempBytes: number;
  unrecognizedBytes: number;
  sqliteFiles: number;
  tempFiles: number;
  unrecognizedFiles: number;
}

export interface StorageArtworkDiskBytes {
  artworkId: string;
  bytes: number;
}

export interface StorageBoardDiskBytes {
  boardId: number;
  bytes: number;
}

export interface StorageMissingFile {
  kind: StorageCategoryKind;
  count: number;
}

export interface RepositoryStorageDiskScan {
  scannedAtMs: number;
  fileCount: number;
  totalBytes: number;
  categories: StorageCategory[];
  otherBreakdown: StorageOtherBreakdown;
  artworkBytes: StorageArtworkDiskBytes[];
  ddsByBoard: StorageBoardDiskBytes[];
  missingFiles: StorageMissingFile[];
}
