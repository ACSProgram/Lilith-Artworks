import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SettingsSnapshot } from "./types";
import type { LibraryTree } from "../modules/library/types";

const appApi = vi.hoisted(() => ({
  getSettings: vi.fn(),
  saveSettings: vi.fn(),
  getRepositoryStatus: vi.fn(),
  retryFileCleanup: vi.fn(),
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
    version: 1,
    repositoryPath,
    theme: "system",
    closeToTray: true,
    pauseAutomaticBackups: false,
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
    appApi.retryFileCleanup.mockResolvedValue({ failures: [] });
    appApi.acknowledgeBackupDisableNotices.mockResolvedValue(undefined);
    appApi.getBackupDisableNoticeTarget.mockResolvedValue(null);
    appApi.getBackupRuntimeStatus.mockResolvedValue({
      busy: false,
      activeBranchId: null,
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
    fireEvent.click(await screen.findByRole("button", { name: "创建备份" }));

    await waitFor(() => {
      expect(appApi.createRepositoryBackup).toHaveBeenCalledWith("C:\\backups");
    });
    expect(await screen.findByText(/备份已校验：4 个文件、2 个历史节点/)).toBeTruthy();
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
      operation: "repository-backup",
      progressLabel: "正在复制仓库文件",
      progressCurrent: 512,
      progressTotal: 1024,
      automaticScheduling: true,
      completionRevision: 0,
    } : {
      busy: false,
      activeBranchId: null,
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
      });
      await backupRequest.promise;
    });
  });

  it("shows release identity and opens the bundled legal directory", async () => {
    const repositoryPath = "C:\\repositories\\A";
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

    expect(await screen.findByText("Lilith Artworks 0.1.0-rc.2")).toBeTruthy();
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
});
