import { invokeCommand } from "../shared/tauri";
import type { CleanupReport } from "../shared/fileCleanup";
import type { AppSettings, RepositoryStatus, SettingsSnapshot } from "./types";

export const appApi = {
  getSettings: () => invokeCommand<SettingsSnapshot>("get_app_settings"),
  saveSettings: (settings: AppSettings) =>
    invokeCommand<SettingsSnapshot>("save_app_settings", { settings }),
  getRepositoryStatus: () => invokeCommand<RepositoryStatus>("get_repository_status"),
  openLogDirectory: () => invokeCommand<void>("open_log_directory"),
  openSettingsDirectory: () => invokeCommand<void>("open_settings_directory"),
  retryFileCleanup: (ids: string[]) =>
    invokeCommand<CleanupReport>("retry_pending_file_cleanup", { ids }),
  acknowledgeBackupDisableNotices: (artworkIds: string[]) =>
    invokeCommand<void>("acknowledge_backup_disable_notices", { artworkIds }),
};
