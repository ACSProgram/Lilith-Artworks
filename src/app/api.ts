import { invokeCommand } from "../shared/tauri";
import type { CleanupReport } from "../shared/fileCleanup";
import type {
  AppSettings,
  BackupDisableNoticeTarget,
  BackupRuntimeStatus,
  RepositoryBackupReport,
  RepositoryScrubReport,
  RepositoryStatus,
  SettingsSnapshot,
} from "./types";

export const appApi = {
  getSettings: () => invokeCommand<SettingsSnapshot>("get_app_settings"),
  saveSettings: (settings: AppSettings) =>
    invokeCommand<SettingsSnapshot>("save_app_settings", { settings }),
  getRepositoryStatus: () => invokeCommand<RepositoryStatus>("get_repository_status"),
  openLogDirectory: () => invokeCommand<void>("open_log_directory"),
  openLegalDirectory: () => invokeCommand<void>("open_legal_directory"),
  openSettingsDirectory: () => invokeCommand<void>("open_settings_directory"),
  retryFileCleanup: (ids: string[]) =>
    invokeCommand<CleanupReport>("retry_pending_file_cleanup", { ids }),
  acknowledgeBackupDisableNotices: (artworkIds: string[]) =>
    invokeCommand<void>("acknowledge_backup_disable_notices", { artworkIds }),
  getBackupDisableNoticeTarget: () =>
    invokeCommand<BackupDisableNoticeTarget | null>("get_backup_disable_notice_target"),
  scrubRepositoryIntegrity: () =>
    invokeCommand<RepositoryScrubReport>("scrub_repository_integrity"),
  createRepositoryBackup: (destinationParent: string) =>
    invokeCommand<RepositoryBackupReport>("create_repository_backup", { destinationParent }),
  getBackupRuntimeStatus: () =>
    invokeCommand<BackupRuntimeStatus>("get_backup_runtime_status"),
  cancelBackupOperation: () =>
    invokeCommand<boolean>("cancel_backup_operation"),
};
