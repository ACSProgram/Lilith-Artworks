import { invokeCommand } from "../../shared/tauri";
import type {
  ArtworkHistory,
  BackupCommitResult,
  BackupRuntimeStatus,
  ForkBranchRequest,
  UpdateBranchBackupRequest,
  RenameHistoryNodeRequest,
} from "./types";

export const historyApi = {
  get: (artworkId: string) =>
    invokeCommand<ArtworkHistory>("get_artwork_history", { artworkId }),
  fork: (request: ForkBranchRequest) =>
    invokeCommand<ArtworkHistory>("fork_artwork_branch", { request }),
  updateBranch: (request: UpdateBranchBackupRequest) =>
    invokeCommand<ArtworkHistory>("update_artwork_branch", { request }),
  renameNode: (request: RenameHistoryNodeRequest) =>
    invokeCommand<ArtworkHistory>("rename_history_node", { request }),
  commit: (branchId: string, note: string) =>
    invokeCommand<BackupCommitResult>("run_branch_backup", {
      request: { branchId, note },
    }),
  restore: (historyId: string, outputPath: string) =>
    invokeCommand<string>("restore_history_node", { historyId, outputPath }),
  compact: (historyId: string) => invokeCommand<void>("compact_history_node", { historyId }),
  deleteSubtree: (historyId: string, branchId: string) =>
    invokeCommand<string>("delete_history_subtree", { historyId, branchId }),
  checkpoint: (historyId: string, enabled: boolean) => invokeCommand<void>("set_history_checkpoint", { historyId, enabled }),
  deleteBranch: (branchId: string) => invokeCommand<ArtworkHistory>("delete_artwork_branch", { branchId }),
  runtime: () => invokeCommand<BackupRuntimeStatus>("get_backup_runtime_status"),
  cancel: () => invokeCommand<boolean>("cancel_backup_operation"),
};
