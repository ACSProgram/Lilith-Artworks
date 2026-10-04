export type Theme = "system" | "light" | "dark";
export type ContentDensity = "comfortable" | "compact";
export type DefaultPanel = "overview" | "history" | "authenticity";
export type PinBoardTextureCacheLevel = "low" | "medium" | "high";
export type BackupCheckMode = "quick" | "full";

export interface PinBoardSettings {
  textureCacheLevel: PinBoardTextureCacheLevel;
  arrangementGapPx: number;
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
  defaultPanel: DefaultPanel;
}

export interface AppSettings {
  version: number;
  repositoryPath: string;
  theme: Theme;
  closeToTray: boolean;
  pauseAutomaticBackups: boolean;
  automaticBackupCheckMode: BackupCheckMode;
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
}

export interface RepositoryStatus {
  configured: boolean;
  ready: boolean;
  rootPath: string;
  databasePath: string;
  error: string | null;
}

/** 仓库完整性检查与整仓备份报告共有的计数。 */
export interface RepositoryIntegrityCounts {
  historyNodes: number;
  finalArtifacts: number;
  certificationRecords: number;
}

export interface RepositoryScrubReport extends RepositoryIntegrityCounts {
  /** 检查的画板图片记录数。 */
  pinBoardImages: number;
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

export interface BackupDisableNoticeTarget {
  artworkId: string;
  branchId: string;
}
