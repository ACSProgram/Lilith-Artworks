import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import packageInfo from "../../package.json";
import { deferred, settings, tree } from "./repositoryTestSupport";

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
  getLastRepositoryBackupRun: vi.fn(async () => null),
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
vi.mock("../modules/authenticity/PublicationView", () => ({
  PublicationView: () => null,
}));
vi.mock("../modules/authenticity/IdentificationView", () => ({
  IdentificationView: () => null,
}));

import { App } from "./App";

describe("App settings and repository page", () => {
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
  });

  it("isolates a cloned repository that reuses artwork and branch IDs", async () => {
    const repositoryA = "C:\\repositories\\A";
    const repositoryB = "C:\\repositories\\B-clone";
    const snapshotA = settings(repositoryA);
    const snapshotB = settings(repositoryB);
    const saveRequest = deferred<ReturnType<typeof settings>>();
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
    fireEvent.click(await screen.findByRole("button", { name: "仓储" }));
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
      pinBoardBitmaps: 5,
      pinBoardMissingDds: 0,
      pinBoardCorruptDds: 0,
      pinBoardOrphanDds: 0,
    });
    libraryApi.listTree.mockResolvedValue(tree("Repository artwork", "C:\\work\\A.psd"));

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓储" }));
    fireEvent.click(await screen.findByRole("button", { name: "开始检查" }));

    expect(
      await screen.findByText(
        "完整性检查通过：3 个历史节点、1 个最终成品、2 条认证记录、5 个画板位图。",
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
      pinBoardBitmaps: 5,
      pinBoardMissingDds: 1,
      pinBoardCorruptDds: 0,
      pinBoardOrphanDds: 2,
    });
    libraryApi.listTree.mockResolvedValue(tree("Repository artwork", "C:\\work\\A.psd"));

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓储" }));
    fireEvent.click(await screen.findByRole("button", { name: "开始检查" }));

    expect(
      await screen.findByText(
        "完整性检查完成，但发现画板 DDS 问题：缺失 1、损坏 0、孤儿 2（共 5 个画板位图）。",
      ),
    ).toBeTruthy();
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

  it("settles the open pin board before releasing the previous repository", async () => {
    const repositoryA = "C:\\repositories\\A";
    const repositoryB = "C:\\repositories\\B";
    const settle = deferred<void>();
    pinBoardLifecycle.preparePinBoardRuntimeChange.mockReturnValue(settle.promise);
    appApi.getSettings.mockResolvedValue(settings(repositoryA));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryA,
      databasePath: `${repositoryA}\\lilith-artworks.sqlite3`,
      error: null,
    });
    appApi.saveSettings.mockResolvedValue(settings(repositoryB));
    libraryApi.listTree.mockResolvedValue(tree("Repository A artwork", "C:\\work\\A.psd"));

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository A artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓储" }));
    fireEvent.change(await screen.findByLabelText("作品仓库路径"), { target: { value: repositoryB } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => expect(pinBoardLifecycle.preparePinBoardRuntimeChange).toHaveBeenCalledOnce());
    // 素材板结算完成之前不得释放旧仓库。
    expect(appApi.saveSettings).not.toHaveBeenCalled();

    await act(async () => {
      settle.resolve();
      await settle.promise;
    });
    await waitFor(() => expect(appApi.saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({ repositoryPath: repositoryB }),
    ));
  });

  it("keeps the settings dialog open and reloads after a revision conflict", async () => {
    const repositoryPath = "C:\\repositories\\A";
    appApi.getSettings.mockResolvedValue(settings(repositoryPath));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryPath,
      databasePath: `${repositoryPath}\\lilith-artworks.sqlite3`,
      error: null,
    });
    appApi.saveSettings.mockRejectedValue(Object.assign(
      new Error("设置已被其他入口修改，请重新加载后再保存"),
      { code: "settings_conflict" },
    ));
    libraryApi.listTree.mockResolvedValue(tree("Repository artwork", "C:\\work\\A.psd"));

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "保存" }));

    expect(await screen.findByText("设置已被其他入口修改，请重新加载后再保存")).toBeTruthy();
    // 冲突后重新读取设置与仓库状态，弹窗保持打开让用户确认后再保存。
    await waitFor(() => expect(appApi.getSettings.mock.calls.length).toBeGreaterThanOrEqual(3));
    expect(screen.getByRole("dialog", { name: "设置" })).toBeTruthy();
  });

  it("blocks the repository switch when the pin board cannot be settled", async () => {
    const repositoryA = "C:\\repositories\\A";
    const repositoryB = "C:\\repositories\\B";
    pinBoardLifecycle.preparePinBoardRuntimeChange.mockRejectedValue(
      new Error("素材板未能在运行时变更前完成结算"),
    );
    appApi.getSettings.mockResolvedValue(settings(repositoryA));
    appApi.getRepositoryStatus.mockResolvedValue({
      configured: true,
      ready: true,
      rootPath: repositoryA,
      databasePath: `${repositoryA}\\lilith-artworks.sqlite3`,
      error: null,
    });
    libraryApi.listTree.mockResolvedValue(tree("Repository A artwork", "C:\\work\\A.psd"));

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository A artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓储" }));
    fireEvent.change(await screen.findByLabelText("作品仓库路径"), { target: { value: repositoryB } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    expect(await screen.findByText("素材板未能在运行时变更前完成结算")).toBeTruthy();
    expect(appApi.saveSettings).not.toHaveBeenCalled();
    // 切换被中止，旧仓库工作区保持可用。
    expect(screen.getByRole("treeitem", { name: /Repository A artwork/ })).toBeTruthy();
  });
});
