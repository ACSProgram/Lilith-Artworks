export type Theme = "system" | "light" | "dark";
export type ContentDensity = "comfortable" | "compact";
export type DefaultPanel = "overview" | "history" | "authenticity";

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
  window: WindowSettings;
  content: ContentSettings;
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

export interface RepositoryScrubReport {
  historyNodes: number;
  finalArtifacts: number;
  certificationRecords: number;
}
