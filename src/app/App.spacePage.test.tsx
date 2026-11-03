import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dragSlideToEnd } from "../shared/slideToConfirmTesting";
import type { RepositoryScrubReport } from "./types";
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
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
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

describe("App space page", () => {
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
  });

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

  async function openSpacePage() {
    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "空间" }));
  }

  it("shows the storage overview and scans disk usage on demand", async () => {
    readyRepository();
    await openSpacePage();

    expect(await screen.findByRole("radio", { name: "数据库记录" })).toBeTruthy();
    await waitFor(() => expect(appApi.getRepositoryStorageOverview).toHaveBeenCalled());
    expect(screen.getAllByText("2.00 KiB").length).toBeGreaterThan(0);
    expect(screen.getAllByText("4.00×").length).toBeGreaterThan(0);
    expect(screen.getAllByText("8.00×").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Repository artwork").length).toBeGreaterThan(0);
    expect(screen.getByText(/Sketches/)).toBeTruthy();
    expect(appApi.scanRepositoryStorageDisk).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: /扫描磁盘占用/ }));
    await waitFor(() => expect(appApi.scanRepositoryStorageDisk).toHaveBeenCalledOnce());

    const diskMode = await screen.findByRole("radio", { name: "磁盘实际" });
    await waitFor(() => expect(diskMode.getAttribute("aria-checked")).toBe("true"));
    expect(await screen.findByText("画板图片（DDS）")).toBeTruthy();
    expect(await screen.findByText(/SQLite \/ WAL：1.00 KiB/)).toBeTruthy();
    expect(screen.getAllByText("2.50 KiB").length).toBeGreaterThan(0);
  });

  it("disables the disk scan and names the running task while another long task runs", async () => {
    readyRepository();
    const scrub = deferred<RepositoryScrubReport>();
    appApi.scrubRepositoryIntegrity.mockReturnValue(scrub.promise);

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
    fireEvent.click(await screen.findByRole("button", { name: "仓储" }));
    fireEvent.click(await screen.findByRole("button", { name: "开始检查" }));
    fireEvent.click(await screen.findByRole("button", { name: "空间" }));

    const scanButton = await screen.findByRole("button", { name: /扫描磁盘占用/ });
    await waitFor(() => expect((scanButton as HTMLButtonElement).disabled).toBe(true));
    expect(screen.getByText(/当前任务：/)).toBeTruthy();

    await act(async () => {
      scrub.resolve({
        historyNodes: 3,
        finalArtifacts: 1,
        certificationRecords: 2,
        pinBoardBitmaps: 5,
        pinBoardMissingDds: 0,
        pinBoardCorruptDds: 0,
        pinBoardOrphanDds: 0,
      });
    });
    await waitFor(() => expect((scanButton as HTMLButtonElement).disabled).toBe(false));
  });

  it("lists the pending cleanup queue and retries a single entry", async () => {
    readyRepository();
    // 调用顺序：启动重放（队列为空，不发写命令）→ 维护面板首次列出 → 重试后刷新。
    appApi.listPendingFileCleanup
      .mockResolvedValueOnce([])
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

    await openSpacePage();

    expect(await screen.findByText("artworks/artwork-1/snapshots/orphan.lbc")).toBeTruthy();
    expect(screen.getByText(/提交释放旧快照/)).toBeTruthy();
    expect(screen.getByText(/上次失败：文件仍被 历史节点 引用，已保留/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(appApi.retryFileCleanup).toHaveBeenCalledWith(["cleanup-1"]));
    await waitFor(() => expect(screen.queryByText("artworks/artwork-1/snapshots/orphan.lbc")).toBeNull());
    expect(await screen.findByText("已清理 1 个待清理文件。")).toBeTruthy();
  });

  it("does not run the startup cleanup replay when the queue is empty", async () => {
    readyRepository();
    appApi.listPendingFileCleanup.mockResolvedValue([]);

    render(<App />);
    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    await waitFor(() => expect(appApi.listPendingFileCleanup).toHaveBeenCalled());

    // 队列为空是启动常态：不该为了重放去抢写锁（那正是启动被 busy 拦住的来源）。
    expect(appApi.retryFileCleanup).not.toHaveBeenCalled();
  });

  it("keeps the repository usable when the startup cleanup replay is refused", async () => {
    readyRepository();
    appApi.listPendingFileCleanup.mockResolvedValue([{
      id: "cleanup-1",
      path: "artworks/artwork-1/snapshots/orphan.lbc",
      pathKind: "repository_file",
      reason: "history_commit_release",
      createdMs: 1,
      lastAttemptMs: 1_700_000_000_000,
      lastError: null,
    }]);
    // 自动备份正持锁：重放被拒，但界面必须已经完全可用。
    appApi.retryFileCleanup.mockRejectedValue(
      Object.assign(new Error("仓库正在写入，请稍后重试"), { code: "repository_busy" }),
    );

    render(<App />);

    await screen.findByRole("treeitem", { name: /Repository artwork/ });
    await waitFor(() => expect(appApi.retryFileCleanup).toHaveBeenCalledWith([]));
    // busy 只留在诊断日志里：不弹提示，也不把仓库停在不可用状态。
    expect(screen.queryByText(/仓库正在写入/)).toBeNull();
    expect(screen.queryByText(/仓库不可用/)).toBeNull();
  });

  it("requires an explicit confirmation before deleting an external file", async () => {
    readyRepository();
    const externalPath = "D:\\exports\\signed.jpg";
    appApi.listPendingFileCleanup.mockResolvedValue([{
      id: "cleanup-external",
      path: externalPath,
      pathKind: "external_file",
      reason: "publish_certification",
      createdMs: 1,
      lastAttemptMs: null,
      lastError: null,
      expectedSha256: "A".repeat(64),
      artworkTitle: "Repository artwork",
    }]);
    appApi.retryFileCleanup.mockResolvedValue({ cleanedCount: 1, pendingCount: 0, failures: [] });

    await openSpacePage();

    // 仓库外条目展示完整路径、所属作品与期望摘要，且不提供「全部重试」入口。
    expect(await screen.findByText(externalPath)).toBeTruthy();
    expect(screen.getByText(/所属作品：Repository artwork/)).toBeTruthy();
    expect(screen.getByText(`期望 SHA-256：${"A".repeat(64)}`)).toBeTruthy();
    const retryAll = screen.getByRole("button", { name: /全部重试/ }) as HTMLButtonElement;
    expect(retryAll.disabled).toBe(true);

    // 点击按钮只打开确认弹窗，不执行删除；必须滑动确认才会真正入队执行。
    fireEvent.click(screen.getByRole("button", { name: /确认删除/ }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm.textContent).toContain(externalPath);
    expect(confirm.textContent).toContain("所属作品：Repository artwork");
    expect(appApi.retryFileCleanup).not.toHaveBeenCalledWith(["cleanup-external"]);

    dragSlideToEnd("删除这个文件");
    await waitFor(() => expect(appApi.retryFileCleanup).toHaveBeenCalledWith(["cleanup-external"]));
  });

  it("scans junk files and cleans the confirmed candidates", async () => {
    readyRepository();
    appApi.scanRepositoryUnreferenced.mockResolvedValue([
      { path: "artworks/artwork-1/snapshots/orphan.lbc", byteSize: 2048, reason: "历史快照未被引用" },
      { path: "artworks/artwork-1/deltas/a-to-b.lbd", byteSize: 1024, reason: "检查点已持有快照，冗余增量" },
    ]);
    appApi.cleanupRepositoryUnreferenced.mockResolvedValue({ cleanedCount: 2, pendingCount: 0, failures: [] });

    await openSpacePage();
    fireEvent.click(await screen.findByRole("button", { name: "开始扫描" }));

    expect(await screen.findByText("发现 2 个可清理文件")).toBeTruthy();
    expect(screen.getByText("artworks/artwork-1/snapshots/orphan.lbc")).toBeTruthy();
    expect(screen.getByText("历史快照未被引用 · 2.00 KiB")).toBeTruthy();
    expect(screen.getByText("检查点已持有快照，冗余增量 · 1.00 KiB")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "清理这些文件" }));
    fireEvent.click(await screen.findByRole("button", { name: "清理 2 个文件" }));

    await waitFor(() => expect(appApi.cleanupRepositoryUnreferenced).toHaveBeenCalledWith([
      "artworks/artwork-1/snapshots/orphan.lbc",
      "artworks/artwork-1/deltas/a-to-b.lbd",
    ]));
    expect(await screen.findByText("已清理 2 个垃圾文件。")).toBeTruthy();
  });
});
