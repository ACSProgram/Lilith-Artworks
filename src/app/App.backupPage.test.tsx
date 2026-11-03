import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RepositoryBackupRun } from "./types";
import {
  deferred, settings, storageDiskScanFixture, storageOverviewFixture, tree,
} from "./repositoryTestSupport";

const appApi = vi.hoisted(() => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
  getRepositoryStatus: vi.fn(),
  listPendingFileCleanup: vi.fn(),
  retryFileCleanup: vi.fn(),
  scanRepositoryUnreferenced: vi.fn(),
  cleanupRepositoryUnreferenced: vi.fn(),
  acknowledgeBackupDisableNotices: vi.fn(),
  getBackupDisableNoticeTarget: vi.fn(),
  scrubRepositoryIntegrity: vi.fn(),
  createRepositoryBackup: vi.fn(),
  getLastRepositoryBackupRun: vi.fn(async (): Promise<RepositoryBackupRun | null> => null),
  getBackupRuntimeStatus: vi.fn(),
  cancelBackupOperation: vi.fn(),
  getRepositoryStorageOverview: vi.fn(),
  scanRepositoryStorageDisk: vi.fn(),
  openSettingsDirectory: vi.fn(),
  openLogDirectory: vi.fn(),
  openLegalDirectory: vi.fn(),
}));

const libraryApi = vi.hoisted(() => ({
  listTree: vi.fn(),
  search: vi.fn(),
  createGroup: vi.fn(),
  createArtwork: vi.fn(),
  renameNode: vi.fn(),
  trashNodes: vi.fn(),
  listTrash: vi.fn(),
  restoreTrash: vi.fn(),
  permanentlyDeleteTrash: vi.fn(),
  emptyTrash: vi.fn(),
  moveNodes: vi.fn(),
}));

const pinBoardLifecycle = vi.hoisted(() => ({
  preparePinBoardRuntimeChange: vi.fn(),
}));

vi.mock("./api", () => ({ appApi }));
vi.mock("../modules/library/api", () => ({ libraryApi }));
vi.mock("../modules/pin-board/lifecycle", () => pinBoardLifecycle);
const dialog = vi.hoisted(() => ({ open: vi.fn() }));

vi.mock("@tauri-apps/plugin-dialog", () => dialog);
vi.mock("./WindowTitleBar", () => ({
  WindowTitleBar: ({ onOpenSettings }: { onOpenSettings: () => void }) => (
    <button type="button" onClick={onOpenSettings}>打开设置</button>
  ),
}));
vi.mock("../modules/history/HistoryModule", () => ({
  HistoryModule: () => null,
}));
vi.mock("../modules/authenticity/PublicationView", () => ({
  PublicationView: () => null,
}));
vi.mock("../modules/authenticity/IdentificationView", () => ({
  IdentificationView: () => null,
}));

import { App } from "./App";

describe("App backup page", () => {
  afterEach(cleanup);

  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
    pinBoardLifecycle.preparePinBoardRuntimeChange.mockResolvedValue(undefined);
    appApi.listPendingFileCleanup.mockResolvedValue([]);
    appApi.retryFileCleanup.mockResolvedValue({ failures: [] });
    appApi.acknowledgeBackupDisableNotices.mockResolvedValue(undefined);
    appApi.getBackupDisableNoticeTarget.mockResolvedValue(null);
    appApi.getBackupRuntimeStatus.mockResolvedValue({
      busy: false,
      activeBranchId: null,
      taskKind: null,
      operation: null,
      progressLabel: null,
      progressCurrent: 0,
      progressTotal: 0,
      automaticScheduling: true,
      completionRevision: 0,
    });
    appApi.cancelBackupOperation.mockResolvedValue(true);
    appApi.getRepositoryStorageOverview.mockResolvedValue(storageOverviewFixture());
    appApi.scanRepositoryStorageDisk.mockResolvedValue(storageDiskScanFixture());
    appApi.createRepositoryBackup.mockResolvedValue({
      backupPath: "C:\\backups\\Lilith-Artworks-backup-1",
      repositoryPath: "C:\\backups\\Lilith-Artworks-backup-1\\repository",
      fileCount: 4,
      totalBytes: 1024,
      historyNodes: 2,
      finalArtifacts: 0,
      certificationRecords: 0,
      reclaimedStagingDirectories: 0,
      failedStagingDirectories: 0,
    });
  });

  async function openBackupPage() {
    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "备份" }));
  }

  function readyRepository() {
    const repositoryPath = "C:\\repositories\\A";
    appApi.getSettings.mockResolvedValue(settings(repositoryPath));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryPath,
      databasePath: `${repositoryPath}\\lilith-artworks.sqlite3`,
      error: null,
    });
    libraryApi.listTree.mockResolvedValue(tree("Repository artwork", "C:\\work\\A.psd"));
  }

  it("creates a verified repository backup in the selected parent directory", async () => {
    readyRepository();
    dialog.open.mockResolvedValue("C:\\backups");

    await openBackupPage();
    fireEvent.click(await screen.findByRole("button", { name: "创建备份" }));

    await waitFor(() => {
      expect(appApi.createRepositoryBackup).toHaveBeenCalledWith("C:\\backups");
    });
    expect(await screen.findByText(/备份已校验：4 个文件、2 个历史节点/)).toBeTruthy();
    expect(await screen.findByText("最近一次结果")).toBeTruthy();
    expect(screen.getAllByText(/Lilith-Artworks-backup-1/).length).toBeGreaterThan(0);
  });

  it("loads the persisted last backup when the backup page opens", async () => {
    readyRepository();
    appApi.getLastRepositoryBackupRun.mockResolvedValue({
      backupPath: "C:\\backups\\Lilith-Artworks-backup-persisted",
      repositoryPath: "C:\\backups\\Lilith-Artworks-backup-persisted\\repository",
      fileCount: 7,
      totalBytes: 4096,
      historyNodes: 3,
      finalArtifacts: 1,
      certificationRecords: 1,
      createdMs: Date.parse("2026-10-20T08:00:00Z"),
    });

    await openBackupPage();

    expect(await screen.findByText("最近一次结果")).toBeTruthy();
    expect(screen.getAllByText(/Lilith-Artworks-backup-persisted/).length).toBeGreaterThan(0);
    expect(appApi.getLastRepositoryBackupRun).toHaveBeenCalled();
  });

  it("does not read the persisted last backup while the repository is not ready", async () => {
    const repositoryPath = "C:\\repositories\\A";
    appApi.getSettings.mockResolvedValue(settings(repositoryPath));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: false,
      rootPath: repositoryPath,
      databasePath: `${repositoryPath}\\lilith-artworks.sqlite3`,
      error: "仓库不可用",
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "备份" }));

    expect(await screen.findByText("尚无备份记录")).toBeTruthy();
    expect(appApi.getLastRepositoryBackupRun).not.toHaveBeenCalled();
  });

  it("reports reclaimed staging directories after a repository backup", async () => {
    readyRepository();
    dialog.open.mockResolvedValue("C:\\backups");
    appApi.createRepositoryBackup.mockResolvedValue({
      backupPath: "C:\\backups\\Lilith-Artworks-backup-1",
      repositoryPath: "C:\\backups\\Lilith-Artworks-backup-1\\repository",
      fileCount: 4,
      totalBytes: 1024,
      historyNodes: 2,
      finalArtifacts: 0,
      certificationRecords: 0,
      reclaimedStagingDirectories: 2,
      failedStagingDirectories: 1,
    });

    await openBackupPage();
    fireEvent.click(await screen.findByRole("button", { name: "创建备份" }));

    expect(await screen.findByText(/并回收 2 个残留暂存目录/)).toBeTruthy();
    expect(screen.getByText(/1 个残留暂存目录未能回收/)).toBeTruthy();
  });

  it("shows repository backup progress and exposes cancellation", async () => {
    const backupRequest = deferred<{
      backupPath: string;
      repositoryPath: string;
      fileCount: number;
      totalBytes: number;
      historyNodes: number;
      finalArtifacts: number;
      certificationRecords: number;
      reclaimedStagingDirectories: number;
      failedStagingDirectories: number;
    }>();
    let backupStarted = false;
    readyRepository();
    appApi.getBackupRuntimeStatus.mockImplementation(async () => backupStarted ? {
      busy: true,
      activeBranchId: null,
      taskKind: "userOperation",
      // 运行状态上报的是运行域（`OperationDomain`），不是命令名。
      operation: "maintenance",
      progressLabel: "正在复制仓库文件",
      progressCurrent: 512,
      progressTotal: 1024,
      automaticScheduling: true,
      completionRevision: 0,
    } : {
      busy: false,
      activeBranchId: null,
      taskKind: null,
      operation: null,
      progressLabel: null,
      progressCurrent: 0,
      progressTotal: 0,
      automaticScheduling: true,
      completionRevision: 0,
    });
    appApi.createRepositoryBackup.mockImplementation(() => {
      backupStarted = true;
      return backupRequest.promise;
    });
    dialog.open.mockResolvedValue("C:\\backups");

    await openBackupPage();
    fireEvent.click(await screen.findByRole("button", { name: "创建备份" }));

    expect(await screen.findByText("正在复制仓库文件")).toBeTruthy();
    expect(screen.getByText("50%")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "取消当前操作" }));
    await waitFor(() => expect(appApi.cancelBackupOperation).toHaveBeenCalledOnce());

    await act(async () => {
      backupRequest.resolve({
        backupPath: "C:\\backups\\Lilith-Artworks-backup-1",
        repositoryPath: "C:\\backups\\Lilith-Artworks-backup-1\\repository",
        fileCount: 4,
        totalBytes: 1024,
        historyNodes: 2,
        finalArtifacts: 0,
        certificationRecords: 0,
        reclaimedStagingDirectories: 0,
        failedStagingDirectories: 0,
      });
      await backupRequest.promise;
    });
  });
});
