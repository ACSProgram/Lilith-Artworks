import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { appApi } from "./api";
import type { RepositoryBackupReport } from "./types";
import type { SettingsController } from "./useSettingsController";
import type { Notify, SettingsOperation } from "./useSettingsOperationRunner";

/**
 * 设置页展示的「最近一次整仓备份」：命令返回的报告加上完成时间。本次运行的结果与
 * 数据库里的历史记录共用这一形状，避免两处各写一份字面量类型。
 */
export type RepositoryBackupResult = { report: RepositoryBackupReport; completedAtMs: number };

export interface RepositoryBackupMaintenance {
  lastRepositoryBackup: RepositoryBackupResult | null;
  backupRepository: () => Promise<void>;
}

/**
 * 备份页：创建整仓备份并展示最近一次结果（含上次运行持久化的记录）。
 */
export function useRepositoryBackupMaintenance(
  settings: SettingsController,
  notify: Notify,
  runSettingsOperation: (operation: SettingsOperation, action: () => Promise<void>) => Promise<void>,
): RepositoryBackupMaintenance {
  const [lastRepositoryBackup, setLastRepositoryBackup] =
    useState<RepositoryBackupResult | null>(null);

  useEffect(() => {
    setLastRepositoryBackup(null);
  }, [settings.repository.rootPath]);

  // 最近一次整仓备份跨重启保留，显示在「备份」页；读取与当前页签无关。
  // 读取失败时保持当前显示（首次打开即空态），不打扰用户。
  const loadLastRepositoryBackup = useCallback(async (): Promise<RepositoryBackupResult | null> => {
    try {
      const run = await appApi.getLastRepositoryBackupRun();
      if (!run) return null;
      return {
        report: {
          backupPath: run.backupPath,
          repositoryPath: run.repositoryPath,
          fileCount: run.fileCount,
          totalBytes: run.totalBytes,
          historyNodes: run.historyNodes,
          finalArtifacts: run.finalArtifacts,
          certificationRecords: run.certificationRecords,
          // 暂存目录回收是本次运行的事实，不写进数据库；历史记录按 0 显示。
          reclaimedStagingDirectories: 0,
          failedStagingDirectories: 0,
        },
        completedAtMs: run.createdMs,
      };
    } catch {
      return null;
    }
  }, []);

  useEffect(() => {
    if (!settings.settingsOpen || !settings.repository.ready) {
      return undefined;
    }
    let disposed = false;
    void loadLastRepositoryBackup().then((loaded) => {
      if (disposed || !loaded) return;
      // 读取期间可能刚完成一次备份：只接受不比当前结果更旧的记录。
      setLastRepositoryBackup((current) => {
        if (current && current.completedAtMs >= loaded.completedAtMs) return current;
        return loaded;
      });
    });
    return () => {
      disposed = true;
    };
  }, [loadLastRepositoryBackup, settings.repository.ready, settings.settingsOpen, settings.repository.rootPath]);

  const backupRepository = useCallback(async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "选择备份保存位置",
    });
    if (typeof selected !== "string") return;
    await runSettingsOperation("repository-backup", async () => {
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
    });
  }, [notify, runSettingsOperation]);

  return { lastRepositoryBackup, backupRepository };
}
