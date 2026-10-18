import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { appApi } from "./api";
import { IDLE_BACKUP_RUNTIME, useRuntimeProgress } from "./RuntimeProgressProvider";
import { errorMessage } from "../shared/errors";
import { formatBytes } from "../shared/format";
import { useRequestEpoch } from "../shared/useRequestEpoch";
import { useRuntimeSubscription } from "../shared/useRuntimeSubscription";
import {
  isExternalCleanupEntry,
  type PendingCleanupEntry,
  type UnreferencedScanCandidate,
} from "../shared/fileCleanup";
import type {
  BackupRuntimeStatus,
  RepositoryBackupReport,
  RepositoryStorageDiskScan,
  RepositoryStorageOverview,
} from "./types";
import type { SettingsController } from "./useSettingsController";

export type SettingsOperation =
  | "repository-scrub"
  | "repository-backup"
  | "unreferenced-scan"
  | "storage-scan";

/**
 * 设置页长任务在后端上报的运行域。
 *
 * 运行状态的 `operation` 是 `OperationDomain` 的序列化值而不是命令名：完整性扫描、
 * 整仓备份与磁盘扫描都属 `maintenance`，未引用扫描属 `cleanup`。据此只认设置页
 * 自己的任务，发布（`authenticity`）、素材板（`pinBoard`）等前台任务与后台调度
 * 任务（`taskKind` 不是 `userOperation`、`operation` 为 `background`）都不会把
 * 进度串到这个弹窗。
 */
const SETTINGS_OPERATION_DOMAINS = new Set(["maintenance", "cleanup"]);

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
  lastRepositoryBackup: { report: RepositoryBackupReport; completedAtMs: number } | null;
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

type Notify = (message: string | null) => void;

/**
 * 仓库维护流程（完整性扫描、整仓备份、未引用文件扫描与待清理队列）的前端编排。
 * 长任务进度经 `RuntimeProgressProvider` 轮询；命令返回的最终状态仍是权威。
 */
export function useRepositoryMaintenance(
  settings: SettingsController,
  notify: Notify,
): RepositoryMaintenance {
  const {
    runtime,
    cancelPending,
    refresh: refreshRuntime,
    setPolling,
    cancel,
  } = useRuntimeProgress();
  const [settingsOperation, setSettingsOperation] = useState<SettingsOperation | null>(null);
  const [pendingCleanup, setPendingCleanup] = useState<PendingCleanupEntry[]>([]);
  const [externalCleanupTarget, setExternalCleanupTarget] = useState<PendingCleanupEntry | null>(null);
  const [scanCandidates, setScanCandidates] = useState<UnreferencedScanCandidate[] | null>(null);
  const [scanConfirmOpen, setScanConfirmOpen] = useState(false);
  const [cleanupBusy, setCleanupBusy] = useState(false);
  const [storageOverview, setStorageOverview] = useState<RepositoryStorageOverview | null>(null);
  const [storageDiskScan, setStorageDiskScan] = useState<RepositoryStorageDiskScan | null>(null);
  const [storageOverviewBusy, setStorageOverviewBusy] = useState(false);
  const [storageOverviewError, setStorageOverviewError] = useState<string | null>(null);
  const [lastRepositoryBackup, setLastRepositoryBackup] = useState<{
    report: RepositoryBackupReport;
    completedAtMs: number;
  } | null>(null);
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
    setLastRepositoryBackup(null);
  }, [invalidateStorageReads, settings.repository.rootPath]);

  useEffect(() => {
    const active = settings.settingsOpen || settingsOperation !== null;
    setPolling(active);
    return () => setPolling(false);
  }, [setPolling, settings.settingsOpen, settingsOperation]);

  // 操作刚启动时立即读一次运行状态：事件会让进度随后持续到达，但首帧
  // 不能等 3 秒兜底轮询。
  useEffect(() => {
    if (settingsOperation !== null) void refreshRuntime().catch(() => undefined);
  }, [refreshRuntime, settingsOperation]);

  // 打开设置或切到仓库页时读取一次待清理队列；只读，失败时保持上一次结果。
  useEffect(() => {
    if (!settings.settingsOpen || settings.settingsPage !== "repository" || !settings.repository.ready) {
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

  const scrubRepository = useCallback(async () => {
    let current: BackupRuntimeStatus;
    try {
      current = await refreshRuntime();
    } catch (error) {
      notify(errorMessage(error));
      return;
    }
    if (current.busy) {
      notify("已有备份操作正在运行，请等待完成或先取消当前操作。");
      return;
    }
    setSettingsOperation("repository-scrub");
    notify(null);
    try {
      const report = await appApi.scrubRepositoryIntegrity();
      const pinBoardIssues =
        report.pinBoardMissingDds + report.pinBoardCorruptDds + report.pinBoardOrphanDds;
      notify(
        pinBoardIssues > 0
          ? `完整性检查完成，但发现画板 DDS 问题：缺失 ${report.pinBoardMissingDds}、损坏 ${report.pinBoardCorruptDds}、孤儿 ${report.pinBoardOrphanDds}（共 ${report.pinBoardImages} 张画板图片）。`
          : `完整性检查通过：${report.historyNodes} 个历史节点、${report.finalArtifacts} 个最终成品、${report.certificationRecords} 条认证记录、${report.pinBoardImages} 张画板图片。`,
      );
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setSettingsOperation(null);
    }
  }, [notify, refreshRuntime]);

  const backupRepository = useCallback(async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "选择备份保存位置",
    });
    if (typeof selected !== "string") return;
    let current: BackupRuntimeStatus;
    try {
      current = await refreshRuntime();
    } catch (error) {
      notify(errorMessage(error));
      return;
    }
    if (current.busy) {
      notify("已有备份操作正在运行，请等待完成或先取消当前操作。");
      return;
    }
    setSettingsOperation("repository-backup");
    notify(null);
    try {
      const report = await appApi.createRepositoryBackup(selected);
      setLastRepositoryBackup({ report, completedAtMs: Date.now() });
      const reclaimed = report.reclaimedStagingDirectories > 0
        ? `，并回收 ${report.reclaimedStagingDirectories} 个残留暂存目录`
        : "";
      const reclaimFailed = report.failedStagingDirectories > 0
        ? `（另有 ${report.failedStagingDirectories} 个残留暂存目录未能回收，不影响本次备份）`
        : "";
      notify(
        `备份已校验：${report.fileCount} 个文件、${report.historyNodes} 个历史节点${reclaimed}${reclaimFailed}。恢复时选择 ${report.repositoryPath}`,
      );
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setSettingsOperation(null);
    }
  }, [notify, refreshRuntime]);

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

  const scanStorageDisk = useCallback(async () => {
    let current: BackupRuntimeStatus;
    try {
      current = await refreshRuntime();
    } catch (error) {
      notify(errorMessage(error));
      return;
    }
    if (current.busy) {
      notify("已有备份操作正在运行，请等待完成或先取消当前操作。");
      return;
    }
    setSettingsOperation("storage-scan");
    notify(null);
    try {
      const scan = await appApi.scanRepositoryStorageDisk();
      setStorageDiskScan(scan);
      await refreshStorageOverview();
      notify(`磁盘占用扫描完成：${scan.fileCount} 个文件，共 ${formatBytes(scan.totalBytes)}。`);
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setSettingsOperation(null);
    }
  }, [notify, refreshRuntime, refreshStorageOverview]);

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

  const scanUnreferenced = useCallback(async () => {
    let current: BackupRuntimeStatus;
    try {
      current = await refreshRuntime();
    } catch (error) {
      notify(errorMessage(error));
      return;
    }
    if (current.busy) {
      notify("已有备份操作正在运行，请等待完成或先取消当前操作。");
      return;
    }
    setSettingsOperation("unreferenced-scan");
    setScanCandidates(null);
    notify(null);
    try {
      const candidates = await appApi.scanRepositoryUnreferenced();
      setScanCandidates(candidates);
      notify(
        candidates.length > 0
          ? `未引用文件扫描完成：发现 ${candidates.length} 个可清理文件，请确认后清理。`
          : "未引用文件扫描完成：没有发现可清理的未引用文件。",
      );
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setSettingsOperation(null);
    }
  }, [notify, refreshRuntime]);

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
        notify(`已清理 ${report.cleanedCount} 个未引用文件。`);
      }
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setCleanupBusy(false);
    }
  }, [notify, refreshPendingCleanup, scanCandidates]);

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
            ? "正在准备未引用文件扫描"
            : settingsOperation === "storage-scan"
              ? "正在准备磁盘占用扫描"
              : "正在准备完整性检查",
      }
      : null;
  const settingsBusy = settings.busy
    || settingsOperation !== null
    || runtime.busy
    || cleanupBusy;
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
    settingsOperation,
    pendingCleanup,
    externalCleanupTarget,
    setExternalCleanupTarget,
    scanCandidates,
    setScanCandidates,
    scanConfirmOpen,
    setScanConfirmOpen,
    cleanupBusy,
    settingsBusy,
    cancelPending,
    visibleSettingsRuntime,
    repositoryCleanupCount,
    pendingCleanupSummary,
    storageOverview,
    storageDiskScan,
    storageOverviewBusy,
    storageOverviewError,
    lastRepositoryBackup,
    scrubRepository,
    backupRepository,
    refreshPendingCleanup,
    retryCleanup,
    scanUnreferenced,
    purgeScanCandidates,
    refreshStorageOverview,
    scanStorageDisk,
    cancelSettingsOperation,
  };
}
