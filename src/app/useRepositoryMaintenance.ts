import { useRepositoryBackupMaintenance } from "./useRepositoryBackupMaintenance";
import type { RepositoryBackupResult } from "./useRepositoryBackupMaintenance";
import { useRepositoryIntegrityMaintenance } from "./useRepositoryIntegrityMaintenance";
import { useRepositorySpaceMaintenance } from "./useRepositorySpaceMaintenance";
import { useSettingsOperationRunner } from "./useSettingsOperationRunner";
import type { Notify, SettingsOperation } from "./useSettingsOperationRunner";
import type { BackupRuntimeStatus } from "../shared/backupRuntime";
import type { PendingCleanupEntry, UnreferencedScanCandidate } from "../shared/fileCleanup";
import type {
  RepositoryStorageDiskScan,
  RepositoryStorageOverview,
} from "./types";
import type { SettingsController } from "./useSettingsController";

export type { RepositoryBackupResult };

export interface RepositoryMaintenance {
  settingsOperation: SettingsOperation | null;
  pendingCleanup: PendingCleanupEntry[];
  externalCleanupTarget: PendingCleanupEntry | null;
  setExternalCleanupTarget: (entry: PendingCleanupEntry | null) => void;
  scanCandidates: UnreferencedScanCandidate[] | null;
  setScanCandidates: (candidates: UnreferencedScanCandidate[] | null) => void;
  scanConfirmOpen: boolean;
  setScanConfirmOpen: (open: boolean) => void;
  cleanupBusy: boolean;
  settingsBusy: boolean;
  cancelPending: boolean;
  visibleSettingsRuntime: BackupRuntimeStatus | null;
  repositoryCleanupCount: number;
  pendingCleanupSummary: string;
  storageOverview: RepositoryStorageOverview | null;
  storageDiskScan: RepositoryStorageDiskScan | null;
  storageOverviewBusy: boolean;
  storageOverviewError: string | null;
  lastRepositoryBackup: RepositoryBackupResult | null;
  scrubRepository: () => Promise<void>;
  backupRepository: () => Promise<void>;
  refreshPendingCleanup: () => Promise<void>;
  retryCleanup: (ids: string[]) => Promise<void>;
  scanUnreferenced: () => Promise<void>;
  purgeScanCandidates: () => Promise<void>;
  refreshStorageOverview: () => Promise<void>;
  scanStorageDisk: () => Promise<void>;
  cancelSettingsOperation: () => Promise<void>;
}

/**
 * 仓库维护流程（完整性扫描、整仓备份、垃圾文件扫描与待清理队列）的前端编排。
 * 三个域 hook 各自管理状态，共享同一个长任务闸门；长任务进度经
 * `RuntimeProgressProvider` 轮询，命令返回的最终状态仍是权威。
 */
export function useRepositoryMaintenance(
  settings: SettingsController,
  notify: Notify,
): RepositoryMaintenance {
  const runner = useSettingsOperationRunner(settings.settingsOpen, notify);
  const space = useRepositorySpaceMaintenance(settings, notify, runner.runSettingsOperation);
  const backup = useRepositoryBackupMaintenance(settings, notify, runner.runSettingsOperation);
  const integrity = useRepositoryIntegrityMaintenance(notify, runner.runSettingsOperation);

  return {
    ...runner,
    ...space,
    ...backup,
    ...integrity,
    settingsBusy: settings.busy
      || runner.settingsOperation !== null
      || runner.runtime.busy
      || space.cleanupBusy,
  };
}
