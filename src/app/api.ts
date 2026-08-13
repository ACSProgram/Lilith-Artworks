import { invokeCommand } from "../shared/tauri";
import type { AppSettings, RepositoryStatus, SettingsSnapshot } from "./types";

export const appApi = {
  getSettings: () => invokeCommand<SettingsSnapshot>("get_app_settings"),
  saveSettings: (settings: AppSettings) =>
    invokeCommand<SettingsSnapshot>("save_app_settings", { settings }),
  getRepositoryStatus: () => invokeCommand<RepositoryStatus>("get_repository_status"),
};

