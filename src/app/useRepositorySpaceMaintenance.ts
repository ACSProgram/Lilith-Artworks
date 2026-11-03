import { useCallback, useEffect, useMemo, useState } from "react";
import { appApi } from "./api";
import { errorMessage } from "../shared/errors";
import { formatBytes } from "../shared/format";
import { useRequestEpoch } from "../shared/useRequestEpoch";
import { useRuntimeSubscription } from "../shared/useRuntimeSubscription";
import {
  isExternalCleanupEntry,
  type PendingCleanupEntry,
  type UnreferencedScanCandidate,
} from "../shared/fileCleanup";
import type { RepositoryStorageDiskScan, RepositoryStorageOverview } from "./types";
import type { SettingsController } from "./useSettingsController";
import type { Notify, SettingsOperation } from "./useSettingsOperationRunner";

export interface RepositorySpaceMaintenance {
  pendingCleanup: PendingCleanupEntry[];
  externalCleanupTarget: PendingCleanupEntry | null;
  setExternalCleanupTarget: (entry: PendingCleanupEntry | null) => void;
  scanCandidates: UnreferencedScanCandidate[] | null;
  setScanCandidates: (candidates: UnreferencedScanCandidate[] | null) => void;
  scanConfirmOpen: boolean;
  setScanConfirmOpen: (open: boolean) => void;
  cleanupBusy: boolean;
  repositoryCleanupCount: number;
  pendingCleanupSummary: string;
  storageOverview: RepositoryStorageOverview | null;
  storageDiskScan: RepositoryStorageDiskScan | null;
  storageOverviewBusy: boolean;
  storageOverviewError: string | null;
  refreshPendingCleanup: () => Promise<void>;
  retryCleanup: (ids: string[]) => Promise<void>;
  scanUnreferenced: () => Promise<void>;
  purgeScanCandidates: () => Promise<void>;
  refreshStorageOverview: () => Promise<void>;
  scanStorageDisk: () => Promise<void>;
}

/**
 * 空间页：仓储记录口径、磁盘扫描、垃圾文件扫描与待清理队列。
 */
export function useRepositorySpaceMaintenance(
  settings: SettingsController,
  notify: Notify,
  runSettingsOperation: (operation: SettingsOperation, action: () => Promise<void>) => Promise<void>,
): RepositorySpaceMaintenance {
  const [pendingCleanup, setPendingCleanup] = useState<PendingCleanupEntry[]>([]);
  const [externalCleanupTarget, setExternalCleanupTarget] = useState<PendingCleanupEntry | null>(null);
  const [scanCandidates, setScanCandidates] = useState<UnreferencedScanCandidate[] | null>(null);
  const [scanConfirmOpen, setScanConfirmOpen] = useState(false);
  const [cleanupBusy, setCleanupBusy] = useState(false);
  const [storageOverview, setStorageOverview] = useState<RepositoryStorageOverview | null>(null);
  const [storageDiskScan, setStorageDiskScan] = useState<RepositoryStorageDiskScan | null>(null);
  const [storageOverviewBusy, setStorageOverviewBusy] = useState(false);
  const [storageOverviewError, setStorageOverviewError] = useState<string | null>(null);
  const {
    begin: beginStorageRead,
    invalidate: invalidateStorageReads,
    isCurrent: isStorageReadCurrent,
  } = useRequestEpoch();

  // 仓库更换后旧仓库的聚合值与磁盘扫描都不再成立；在途读取一并作废，
  // 避免旧仓库的结果在新仓库就绪后回填。等待新仓库 ready 再读取。
  useEffect(() => {
    invalidateStorageReads();
    setStorageOverview(null);
    setStorageDiskScan(null);
    setStorageOverviewBusy(false);
    setStorageOverviewError(null);
  }, [invalidateStorageReads, settings.repository.rootPath]);

  // 打开设置或切到空间页时读取一次待清理队列；只读，失败时保持上一次结果。
  useEffect(() => {
    if (!settings.settingsOpen || settings.settingsPage !== "space" || !settings.repository.ready) {
      return undefined;
    }
    let disposed = false;
    void appApi.listPendingFileCleanup()
      .then((entries) => {
        if (!disposed) setPendingCleanup(entries);
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
    };
  }, [settings.repository.ready, settings.settingsOpen, settings.settingsPage]);

  const refreshPendingCleanup = useCallback(async () => {
    if (!settings.repository.ready) {
      setPendingCleanup([]);
      return;
    }
    try {
      setPendingCleanup(await appApi.listPendingFileCleanup());
    } catch {
      // 队列读取失败保持上一次结果；重试/清理的结果已由命令返回值给出。
    }
  }, [settings.repository.ready]);

  // 提交、删除、结算与清理命令会广播队列变化；设置页打开时据此即时刷新，
  // 不依赖操作结束后的手动调用。
  useRuntimeSubscription((event) => {
    if (event.type === "cleanupQueueChanged") void refreshPendingCleanup();
  }, { enabled: settings.settingsOpen && settings.repository.ready });

  const refreshStorageOverview = useCallback(async () => {
    if (!settings.repository.ready) {
      setStorageOverview(null);
      setStorageOverviewError(null);
      return;
    }
    const epoch = beginStorageRead();
    setStorageOverviewBusy(true);
    try {
      const overview = await appApi.getRepositoryStorageOverview();
      if (!isStorageReadCurrent(epoch)) return;
      setStorageOverview(overview);
      setStorageOverviewError(null);
    } catch (error) {
      if (!isStorageReadCurrent(epoch)) return;
      setStorageOverviewError(errorMessage(error));
    } finally {
      // 只有仍属当前代次的读取才能收掉忙碌态：作废后的收尾交给新读取或仓库切换。
      if (isStorageReadCurrent(epoch)) setStorageOverviewBusy(false);
    }
  }, [beginStorageRead, isStorageReadCurrent, settings.repository.ready]);

  const scanStorageDisk = useCallback(() => runSettingsOperation(
    "storage-scan",
    async () => {
      const scan = await appApi.scanRepositoryStorageDisk();
      setStorageDiskScan(scan);
      await refreshStorageOverview();
      notify(`磁盘占用扫描完成：${scan.fileCount} 个文件，共 ${formatBytes(scan.totalBytes)}。`);
    },
  ), [notify, refreshStorageOverview, runSettingsOperation]);

  const retryCleanup = useCallback(async (ids: string[]) => {
    setCleanupBusy(true);
    notify(null);
    try {
      const report = await appApi.retryFileCleanup(ids);
      await refreshPendingCleanup();
      if (report.failures.length > 0) {
        notify(`已清理 ${report.cleanedCount} 个文件，仍有 ${report.failures.length} 个文件无法删除：${report.failures[0].error}`);
      } else if (report.cleanedCount > 0) {
        notify(`已清理 ${report.cleanedCount} 个待清理文件。`);
      } else {
        notify("待清理队列已为空。");
      }
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setCleanupBusy(false);
    }
  }, [notify, refreshPendingCleanup]);

  const scanUnreferenced = useCallback(() => runSettingsOperation(
    "unreferenced-scan",
    async () => {
      setScanCandidates(null);
      const candidates = await appApi.scanRepositoryUnreferenced();
      setScanCandidates(candidates);
      notify(
        candidates.length > 0
          ? `垃圾文件扫描完成：发现 ${candidates.length} 个可清理文件，请确认后清理。`
          : "垃圾文件扫描完成：没有发现可清理的文件。",
      );
    },
  ), [notify, runSettingsOperation]);

  const purgeScanCandidates = useCallback(async () => {
    if (!scanCandidates || scanCandidates.length === 0) return;
    const paths = scanCandidates.map((candidate) => candidate.path);
    setScanConfirmOpen(false);
    setCleanupBusy(true);
    notify(null);
    try {
      const report = await appApi.cleanupRepositoryUnreferenced(paths);
      setScanCandidates(null);
      await refreshPendingCleanup();
      if (report.failures.length > 0) {
        notify(`已清理 ${report.cleanedCount} 个文件，${report.failures.length} 个文件未能删除，已加入待清理队列可重试。`);
      } else {
        notify(`已清理 ${report.cleanedCount} 个垃圾文件。`);
      }
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setCleanupBusy(false);
    }
  }, [notify, refreshPendingCleanup, scanCandidates]);

  /** 仓库内条目由启动重放与「全部重试」处理；外部条目只走逐条确认。 */
  const repositoryCleanupCount = pendingCleanup
    .filter((entry) => !isExternalCleanupEntry(entry)).length;
  const pendingCleanupSummary = useMemo(() => {
    if (!settings.repository.ready) return "仓库不可用";
    if (pendingCleanup.length === 0) return "没有待清理的文件";
    const failed = pendingCleanup.filter((entry) => entry.lastError).length;
    const external = pendingCleanup.length - repositoryCleanupCount;
    const externalNote = external > 0
      ? `，另有 ${external} 个仓库外文件需逐条确认后删除`
      : "";
    return `${pendingCleanup.length} 个文件待清理${
      failed > 0 ? `，其中 ${failed} 个上次删除失败` : ""
    }${externalNote}`;
  }, [pendingCleanup, repositoryCleanupCount, settings.repository.ready]);

  return {
    pendingCleanup,
    externalCleanupTarget,
    setExternalCleanupTarget,
    scanCandidates,
    setScanCandidates,
    scanConfirmOpen,
    setScanConfirmOpen,
    cleanupBusy,
    repositoryCleanupCount,
    pendingCleanupSummary,
    storageOverview,
    storageDiskScan,
    storageOverviewBusy,
    storageOverviewError,
    refreshPendingCleanup,
    retryCleanup,
    scanUnreferenced,
    purgeScanCandidates,
    refreshStorageOverview,
    scanStorageDisk,
  };
}
