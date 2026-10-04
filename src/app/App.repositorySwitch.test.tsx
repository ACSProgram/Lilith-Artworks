import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import packageInfo from "../../package.json";
import type { SettingsSnapshot } from "./types";
import type { LibraryTree } from "../modules/library/types";

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
  getBackupRuntimeStatus: vi.fn(),
  cancelBackupOperation: vi.fn(),
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

vi.mock("./api", () => ({ appApi }));
vi.mock("../modules/library/api", () => ({ libraryApi }));
const dialog = vi.hoisted(() => ({ open: vi.fn() }));

vi.mock("@tauri-apps/plugin-dialog", () => dialog);
vi.mock("./WindowTitleBar", () => ({
  WindowTitleBar: ({ onOpenSettings }: { onOpenSettings: () => void }) => (
    <button type="button" onClick={onOpenSettings}>打开设置</button>
  ),
}));
vi.mock("../modules/history/HistoryModule", () => ({
  HistoryModule: ({ selectedBranchId, onSelectBranch }: {
    selectedBranchId: string | null;
    onSelectBranch: (branchId: string) => void;
  }) => (
    <div>
      <span data-testid="selected-branch">{selectedBranchId ?? "none"}</span>
      <button type="button" onClick={() => onSelectBranch("shared-branch-id")}>选择共享分支</button>
    </div>
  ),
}));
vi.mock("../modules/authenticity/AuthenticityModule", () => ({
  AuthenticityModule: () => null,
}));

import { App } from "./App";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

const settings = (repositoryPath: string): SettingsSnapshot => ({
  settings: {
    version: 2,
    repositoryPath,
    theme: "system",
    closeToTray: true,
    pauseAutomaticBackups: false,
    automaticBackupCheckMode: "quick",
    window: {
      x: null,
      y: null,
      width: 1200,
      height: 800,
      maximized: false,
    },
    content: {
      density: "comfortable",
      defaultPanel: "overview",
    },
    pinBoard: {
      textureCacheLevel: "medium",
      arrangementGapPx: 10,
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
});

const tree = (title: string, sourcePath: string, backupDisableNoticeCount = 0): LibraryTree => ({
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

describe("App repository switching", () => {
  afterEach(cleanup);

  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
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

  it("isolates a cloned repository that reuses artwork and branch IDs", async () => {
    const repositoryA = "C:\\repositories\\A";
    const repositoryB = "C:\\repositories\\B-clone";
    const snapshotA = settings(repositoryA);
    const snapshotB = settings(repositoryB);
    const saveRequest = deferred<SettingsSnapshot>();
    const oldSearch = deferred<Array<{
      id: string;
      kind: "artwork";
      title: string;
      breadcrumb: string;
      ancestorIds: string[];
      sourcePath: string;
    }>>();
    let activeRepository: "A" | "B" = "A";

    appApi.getSettings.mockResolvedValue(snapshotA);
    appApi.getRepositoryStatus.mockImplementation(async () => ({
      configured: true,
      ready: true,
      rootPath: activeRepository === "A" ? repositoryA : repositoryB,
      databasePath: `${activeRepository === "A" ? repositoryA : repositoryB}\\lilith-artworks.sqlite3`,
      error: null,
    }));
    appApi.saveSettings.mockImplementation(() => saveRequest.promise);
    libraryApi.listTree.mockImplementation(async () => activeRepository === "A"
      ? tree("Repository A artwork", "C:\\work\\A.psd")
      : tree("Repository B artwork", "C:\\work\\B.psd"));
    libraryApi.search.mockReturnValue(oldSearch.promise);

    render(<App />);

    const artworkA = await screen.findByRole("treeitem", { name: /Repository A artwork/ });
    fireEvent.click(artworkA);
    fireEvent.click(screen.getByRole("button", { name: "选择共享分支" }));
    expect(screen.getByTestId("selected-branch").textContent).toBe("shared-branch-id");

    fireEvent.change(screen.getByPlaceholderText("搜索标题或工作文件"), {
      target: { value: "only-in-a" },
    });
    await waitFor(() => expect(libraryApi.search).toHaveBeenCalledWith("only-in-a"));

    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓库与备份" }));
    const repositoryInput = await screen.findByLabelText("作品仓库路径");
    fireEvent.change(repositoryInput, { target: { value: repositoryB } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(screen.queryByText("Repository A artwork")).toBeNull();
      expect(screen.queryByTestId("selected-branch")).toBeNull();
    });

    activeRepository = "B";
    saveRequest.resolve(snapshotB);

    const artworkB = await screen.findByRole("treeitem", { name: /Repository B artwork/ });
    expect(screen.queryByText("Repository A artwork")).toBeNull();
    expect(screen.queryByTestId("selected-branch")).toBeNull();

    oldSearch.resolve([{
      id: "shared-artwork-id",
      kind: "artwork",
      title: "Stale repository A result",
      breadcrumb: "Repository A",
      ancestorIds: [],
      sourcePath: "C:\\work\\A.psd",
    }]);
    await Promise.resolve();
    expect(screen.queryByText("Stale repository A result")).toBeNull();

    fireEvent.click(artworkB);
    expect(screen.getByTestId("selected-branch").textContent).toBe("none");
    expect((screen.getByPlaceholderText("搜索标题或工作文件") as HTMLInputElement).value).toBe("");
  });

  it("creates a verified repository backup in the selected parent directory", async () => {
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
    dialog.open.mockResolvedValue("C:\\backups");

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓库与备份" }));
    fireEvent.click(await screen.findByRole("button", { name: "创建备份" }));

    await waitFor(() => {
      expect(appApi.createRepositoryBackup).toHaveBeenCalledWith("C:\\backups");
    });
    expect(await screen.findByText(/备份已校验：4 个文件、2 个历史节点/)).toBeTruthy();
  });

  it("reports reclaimed staging directories after a repository backup", async () => {
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

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓库与备份" }));
    fireEvent.click(await screen.findByRole("button", { name: "创建备份" }));

    expect(await screen.findByText(/并回收 2 个残留暂存目录/)).toBeTruthy();
    expect(screen.getByText(/1 个残留暂存目录未能回收/)).toBeTruthy();
  });

  it("shows repository backup progress and exposes cancellation", async () => {
    const repositoryPath = "C:\\repositories\\A";
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
    appApi.getSettings.mockResolvedValue(settings(repositoryPath));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryPath,
      databasePath: `${repositoryPath}\\lilith-artworks.sqlite3`,
      error: null,
    });
    appApi.getBackupRuntimeStatus.mockImplementation(async () => backupStarted ? {
      busy: true,
      activeBranchId: null,
      taskKind: "userOperation",
      operation: "repository-backup",
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
    libraryApi.listTree.mockResolvedValue(tree("Repository artwork", "C:\\work\\A.psd"));
    dialog.open.mockResolvedValue("C:\\backups");

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓库与备份" }));
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

  it("shows the repository scrub report including pin-board DDS counts", async () => {
    const repositoryPath = "C:\\repositories\\A";
    appApi.getSettings.mockResolvedValue(settings(repositoryPath));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryPath,
      databasePath: `${repositoryPath}\\lilith-artworks.sqlite3`,
      error: null,
    });
    appApi.scrubRepositoryIntegrity.mockResolvedValue({
      historyNodes: 3,
      finalArtifacts: 1,
      certificationRecords: 2,
      pinBoardImages: 5,
      pinBoardMissingDds: 0,
      pinBoardCorruptDds: 0,
      pinBoardOrphanDds: 0,
    });
    libraryApi.listTree.mockResolvedValue(tree("Repository artwork", "C:\\work\\A.psd"));

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓库与备份" }));
    fireEvent.click(await screen.findByRole("button", { name: "开始检查" }));

    expect(
      await screen.findByText(
        "完整性检查通过：3 个历史节点、1 个最终成品、2 条认证记录、5 张画板图片。",
      ),
    ).toBeTruthy();
  });

  it("warns when the repository scrub reports pin-board DDS problems", async () => {
    const repositoryPath = "C:\\repositories\\A";
    appApi.getSettings.mockResolvedValue(settings(repositoryPath));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryPath,
      databasePath: `${repositoryPath}\\lilith-artworks.sqlite3`,
      error: null,
    });
    appApi.scrubRepositoryIntegrity.mockResolvedValue({
      historyNodes: 3,
      finalArtifacts: 1,
      certificationRecords: 2,
      pinBoardImages: 5,
      pinBoardMissingDds: 1,
      pinBoardCorruptDds: 0,
      pinBoardOrphanDds: 2,
    });
    libraryApi.listTree.mockResolvedValue(tree("Repository artwork", "C:\\work\\A.psd"));

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓库与备份" }));
    fireEvent.click(await screen.findByRole("button", { name: "开始检查" }));

    expect(
      await screen.findByText(
        "完整性检查完成，但发现画板 DDS 问题：缺失 1、损坏 0、孤儿 2（共 5 张画板图片）。",
      ),
    ).toBeTruthy();
  });

  it("shows release identity and opens the bundled legal directory", async () => {    const repositoryPath = "C:\\repositories\\A";
    appApi.getSettings.mockResolvedValue(settings(repositoryPath));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryPath,
      databasePath: `${repositoryPath}\\lilith-artworks.sqlite3`,
      error: null,
    });
    appApi.openLegalDirectory.mockResolvedValue(undefined);
    libraryApi.listTree.mockResolvedValue(tree("Repository artwork", "C:\\work\\A.psd"));

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));

    expect(await screen.findByText(`Lilith Artworks ${packageInfo.version}`)).toBeTruthy();
    expect(screen.getByText(/Copyright 2026 ACSProgram/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "查看许可" }));
    await waitFor(() => expect(appApi.openLegalDirectory).toHaveBeenCalledOnce());
  });

  it("opens the affected branch without acknowledging its backup notice", async () => {
    const repositoryPath = "C:\\repositories\\A";
    appApi.getSettings.mockResolvedValue(settings(repositoryPath));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryPath,
      databasePath: `${repositoryPath}\\lilith-artworks.sqlite3`,
      error: null,
    });
    appApi.getBackupDisableNoticeTarget.mockResolvedValue({
      artworkId: "shared-artwork-id",
      branchId: "failed-branch-id",
    });
    libraryApi.listTree.mockResolvedValue(tree(
      "Repository artwork",
      "C:\\work\\A.psd",
      1,
    ));

    render(<App />);
    await screen.findByText("1 个分支的自动备份已关闭");
    fireEvent.click(screen.getByRole("button", { name: "查看分支设置" }));

    await waitFor(() => {
      expect(appApi.getBackupDisableNoticeTarget).toHaveBeenCalledOnce();
      expect(screen.getByTestId("selected-branch").textContent).toBe("failed-branch-id");
    });
    expect(appApi.acknowledgeBackupDisableNotices).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "知道了" }));
    await waitFor(() => expect(appApi.acknowledgeBackupDisableNotices)
      .toHaveBeenCalledWith(["shared-artwork-id"]));
  });

  it("lists the pending cleanup queue and retries a single entry", async () => {
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
    appApi.listPendingFileCleanup
      .mockResolvedValueOnce([{
        id: "cleanup-1",
        path: "artworks/artwork-1/snapshots/orphan.lbc",
        pathKind: "repository_file",
        reason: "history_commit_release",
        createdMs: 1,
        lastAttemptMs: 1_700_000_000_000,
        lastError: "文件仍被 历史节点 引用，已保留",
      }])
      .mockResolvedValue([]);
    appApi.retryFileCleanup.mockResolvedValue({ cleanedCount: 1, pendingCount: 0, failures: [] });

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓库与备份" }));

    expect(await screen.findByText("artworks/artwork-1/snapshots/orphan.lbc")).toBeTruthy();
    expect(screen.getByText(/提交释放旧快照/)).toBeTruthy();
    expect(screen.getByText(/上次失败：文件仍被 历史节点 引用，已保留/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(appApi.retryFileCleanup).toHaveBeenCalledWith(["cleanup-1"]));
    await waitFor(() => expect(screen.queryByText("artworks/artwork-1/snapshots/orphan.lbc")).toBeNull());
    expect(await screen.findByText("已清理 1 个待清理文件。")).toBeTruthy();
  });

  it("scans unreferenced files and cleans the confirmed candidates", async () => {
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
    appApi.scanRepositoryUnreferenced.mockResolvedValue([
      { path: "artworks/artwork-1/snapshots/orphan.lbc", byteSize: 2048, reason: "历史快照未被引用" },
      { path: "artworks/artwork-1/deltas/a-to-b.lbd", byteSize: 1024, reason: "历史增量未被引用" },
    ]);
    appApi.cleanupRepositoryUnreferenced.mockResolvedValue({ cleanedCount: 2, pendingCount: 0, failures: [] });

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓库与备份" }));
    fireEvent.click(await screen.findByRole("button", { name: "开始扫描" }));

    expect(await screen.findByText("发现 2 个未引用文件")).toBeTruthy();
    expect(screen.getByText("artworks/artwork-1/snapshots/orphan.lbc")).toBeTruthy();
    expect(screen.getByText("历史快照未被引用 · 2.00 KiB")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "清理这些文件" }));
    fireEvent.click(await screen.findByRole("button", { name: "清理 2 个文件" }));

    await waitFor(() => expect(appApi.cleanupRepositoryUnreferenced).toHaveBeenCalledWith([
      "artworks/artwork-1/snapshots/orphan.lbc",
      "artworks/artwork-1/deltas/a-to-b.lbd",
    ]));
    expect(await screen.findByText("已清理 2 个未引用文件。")).toBeTruthy();
  });
});
