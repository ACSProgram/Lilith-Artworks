import type {
  RepositoryStorageDiskScan,
  RepositoryStorageOverview,
  SettingsSnapshot,
} from "./types";
import type { LibraryTree } from "../modules/library/types";

export function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

export const settings = (repositoryPath: string): SettingsSnapshot => ({
  settings: {
    version: 2,
    revision: 0,
    repositoryPath,
    theme: "system",
    closeToTray: true,
    pauseAutomaticBackups: false,
    idleIntegrityCheckEnabled: false,
    automaticBackupCheckMode: "quick",
    chunkFileFormatV2: true,
    window: {
      x: null,
      y: null,
      width: 1200,
      height: 800,
      maximized: false,
    },
    content: {
      density: "comfortable",
    },
    pinBoard: {
      textureCacheLevel: "medium",
      arrangementGapRatio: 0.06,
      autosave: false,
      saveOnExit: true,
      lockShortcut: "CommandOrControl+R",
      fullscreenShortcut: "F11",
    },
  },
  settingsPath: "C:\\settings\\settings.json",
  logDirectory: "C:\\settings\\logs",
  warning: null,
  automaticBackupFileCount: 1,
  idleIntegrityPendingCount: 0,
});

export const tree = (title: string, sourcePath: string, backupDisableNoticeCount = 0): LibraryTree => ({
  nodes: [{
    id: "shared-artwork-id",
    parentId: null,
    kind: "artwork",
    title,
    position: 0,
    updatedMs: 1,
    children: [],
    artwork: {
      description: "",
      branchCount: 1,
      backupDisableNoticeCount,
      primaryBranch: {
        id: "shared-branch-id",
        title: "Main",
        sourcePath,
      },
    },
  }],
  groupCount: 0,
  artworkCount: 1,
});

export const storageOverviewFixture = (): RepositoryStorageOverview => ({
  calculatedAtMs: 1,
  databaseRecordedBytes: 2048,
  categories: [
    { kind: "snapshot", bytes: 1024, fileCount: 2 },
    { kind: "delta", bytes: 1024, fileCount: 3 },
    { kind: "finalArtifact", bytes: 0, fileCount: 0 },
    { kind: "certification", bytes: 0, fileCount: 0 },
  ],
  compression: {
    snapshot: { logicalBytes: 4096, storedBytes: 1024, ratio: 4 },
    delta: { logicalBytes: 8192, storedBytes: 1024, ratio: 8 },
  },
  artworks: [{
    artworkId: "shared-artwork-id",
    title: "Repository artwork",
    groupPath: ["Sketches"],
    trashed: false,
    snapshotBytes: 1024,
    deltaBytes: 1024,
    finalArtifactBytes: 0,
    certificationBytes: 0,
    recordedBytes: 2048,
    nodeCount: 1,
    branchCount: 1,
    branches: [{
      branchId: "shared-branch-id",
      title: "Main",
      snapshotBytes: 1024,
      deltaBytes: 1024,
      finalArtifactBytes: 0,
      certificationBytes: 0,
      recordedBytes: 2048,
      finalArtifact: null,
      certificationRecords: [],
      nodes: [{
        id: "node-1",
        title: "First snapshot",
        createdMs: 1,
        isCheckpoint: true,
        hasSnapshot: true,
        logicalSize: 4096,
        chunkFileSize: 1024,
        chunkCount: 4,
        compressionRatio: 4,
        snapshotPath: "artworks/shared-artwork-id/snapshots/node-1.lbc",
        deltaPath: null,
      }],
    }],
    boards: [],
  }],
  unassigned: [],
});

export const storageDiskScanFixture = (): RepositoryStorageDiskScan => ({
  scannedAtMs: 2,
  fileCount: 5,
  totalBytes: 4096,
  categories: [
    { kind: "snapshot", bytes: 1024, fileCount: 1 },
    { kind: "delta", bytes: 1024, fileCount: 1 },
    { kind: "finalArtifact", bytes: 0, fileCount: 0 },
    { kind: "certification", bytes: 0, fileCount: 0 },
    { kind: "boardDds", bytes: 512, fileCount: 1 },
    { kind: "other", bytes: 1536, fileCount: 2 },
  ],
  otherBreakdown: {
    sqliteBytes: 1024,
    tempBytes: 512,
    unrecognizedBytes: 0,
    sqliteFiles: 1,
    tempFiles: 1,
    unrecognizedFiles: 0,
  },
  artworkBytes: [{ artworkId: "shared-artwork-id", bytes: 2560 }],
  ddsByBoard: [],
  missingFiles: [],
});
