import type { AppSettings } from "../types";

export type SettingsUpdate = (patch: Partial<AppSettings>) => void;

export type OpenSettingsFolder = (
  command: "openSettingsDirectory" | "openLogDirectory" | "openLegalDirectory",
) => Promise<void>;
