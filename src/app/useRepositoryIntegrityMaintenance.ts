import { useCallback } from "react";
import { appApi } from "./api";
import type { Notify, SettingsOperation } from "./useSettingsOperationRunner";

export interface RepositoryIntegrityMaintenance {
  scrubRepository: () => Promise<void>;
}

/** 完整性页：仓库完整性扫描与画板 DDS 结果提示。 */
export function useRepositoryIntegrityMaintenance(
  notify: Notify,
  runSettingsOperation: (operation: SettingsOperation, action: () => Promise<void>) => Promise<void>,
): RepositoryIntegrityMaintenance {
  const scrubRepository = useCallback(() => runSettingsOperation(
    "repository-scrub",
    async () => {
      const report = await appApi.scrubRepositoryIntegrity();
      const pinBoardIssues =
        report.pinBoardMissingDds + report.pinBoardCorruptDds + report.pinBoardOrphanDds;
      notify(
        pinBoardIssues > 0
          ? `完整性检查完成，但发现画板 DDS 问题：缺失 ${report.pinBoardMissingDds}、损坏 ${report.pinBoardCorruptDds}、孤儿 ${report.pinBoardOrphanDds}（共 ${report.pinBoardBitmaps} 个画板位图）。`
          : `完整性检查通过：${report.historyNodes} 个历史节点、${report.finalArtifacts} 个最终成品、${report.certificationRecords} 条认证记录、${report.pinBoardBitmaps} 个画板位图。`,
      );
    },
  ), [notify, runSettingsOperation]);

  return { scrubRepository };
}
