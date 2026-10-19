import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dragSlideToEnd } from "../../shared/slideToConfirmTesting";
import { HistoryModule } from "./HistoryModule";
import type { ArtworkBranch, ArtworkHistory, BackupRuntimeStatus, HistoryNode } from "./types";

const controller = vi.hoisted(() => ({ useHistoryController: vi.fn() }));
vi.mock("./useHistoryController", () => controller);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const idleRuntime: BackupRuntimeStatus = {
  busy: false,
  activeBranchId: null,
  taskKind: null,
  operation: null,
  progressLabel: null,
  progressCurrent: 0,
  progressTotal: 0,
  automaticScheduling: false,
  completionRevision: 0,
};

function node(id: string, title: string, parentId: string | null, branchId: string, createdMs: number): HistoryNode {
  return {
    id,
    createdOnBranchId: branchId,
    parentId,
    title,
    note: "",
    commitKind: "manual",
    isCheckpoint: false,
    createdMs,
    logicalSize: 100,
    chunkFileSize: 50,
    sha256: "a".repeat(64),
    chunkCount: 1,
  };
}

const branches: ArtworkBranch[] = [
  {
    id: "main",
    title: "Main",
    sourcePath: "C:\\work\\main.psd",
    headHistoryId: "main-head",
    createdFromHistoryId: null,
    backupEnabled: true,
    backupIntervalMinutes: 10,
    backupQuickEnabled: false,
    lastCheckMs: null,
    lastSuccessMs: null,
    lastError: null,
    consecutiveBackupFailures: 0,
    backupRetryAtMs: null,
    backupDisableNoticePending: false,
    finalArtifactLocked: false,
    publishedCount: 0,
    verifyError: null,
    verifiedMs: null,
  },
  {
    id: "other",
    title: "Other",
    sourcePath: "C:\\work\\other.psd",
    headHistoryId: "other-head",
    createdFromHistoryId: "root",
    backupEnabled: true,
    backupIntervalMinutes: 10,
    backupQuickEnabled: false,
    lastCheckMs: null,
    lastSuccessMs: null,
    lastError: null,
    consecutiveBackupFailures: 0,
    backupRetryAtMs: null,
    backupDisableNoticePending: false,
    finalArtifactLocked: false,
    publishedCount: 0,
    verifyError: null,
    verifiedMs: null,
  },
];

const history: ArtworkHistory = {
  artworkId: "artwork-1",
  artworkTitle: "Artwork",
  branches,
  nodes: [
    node("root", "Root", null, "main", 1),
    node("middle", "Middle", "root", "main", 2),
    node("main-head", "Main head", "middle", "main", 3),
    node("other-head", "Other head", "root", "other", 4),
  ],
};

/** 与默认标题一致、未改名的自动备份节点：精简时属于可安全删除的那一类。 */
const automaticHistory: ArtworkHistory = {
  ...history,
  nodes: [
    node("root", "Root", null, "main", 1),
    { ...node("auto", "自动备份", "root", "main", 2), commitKind: "automatic" },
    node("main-head", "Main head", "auto", "main", 3),
    node("other-head", "Other head", "root", "other", 4),
  ],
};

function controllerReturn(overrides: Record<string, unknown> = {}) {
  return {
    history,
    loading: false,
    busy: false,
    runtime: idleRuntime,
    visibleRuntime: idleRuntime,
    saveBranch: vi.fn().mockResolvedValue(undefined),
    commitBranch: vi.fn(),
    restoreNode: vi.fn(),
    compactNodes: vi.fn(),
    deleteBranch: vi.fn(),
    reverifyBranch: vi.fn(),
    deleteSubtree: vi.fn(),
    setCheckpoint: vi.fn(),
    forkBranch: vi.fn(),
    renameNode: vi.fn(),
    readNodeFormat: vi.fn().mockResolvedValue("v2"),
    readNodeThumbnail: vi.fn().mockResolvedValue(null),
    cancelOperation: vi.fn(),
    ...overrides,
  };
}

describe("HistoryModule overview interactions", () => {
  beforeEach(() => {
    controller.useHistoryController.mockReturnValue(controllerReturn());
  });

  afterEach(() => cleanup());

  it("enters compact mode without leaving the overview", () => {
    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "精简当前分支" }));
    expect(screen.getByText("精简 Main")).toBeTruthy();
    expect(screen.getByRole("button", { name: "总览" }).classList.contains("active")).toBe(true);

    fireEvent.click(screen.getByText("Middle").closest("button")!);
    expect(screen.getByText("1 个已选")).toBeTruthy();
  });

  it("selects a uniquely containing branch on double click without changing views", () => {
    const onSelectBranch = vi.fn();
    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={onSelectBranch}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.doubleClick(screen.getByText("Other head").closest("button")!);
    expect(onSelectBranch).toHaveBeenCalledWith("other");
    expect(screen.getByRole("button", { name: "总览" }).classList.contains("active")).toBe(true);
  });

  it("requires a second confirmation before removing manual or renamed nodes", async () => {
    const compactNodes = vi.fn().mockResolvedValue(true);
    controller.useHistoryController.mockReturnValue(controllerReturn({ compactNodes }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "精简当前分支" }));
    fireEvent.click(screen.getByText("Middle").closest("button")!);
    fireEvent.click(screen.getByRole("button", { name: "删除节点" }));

    expect(screen.getByRole("heading", { name: "删除节点" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "确认删除" }));

    expect(screen.getByRole("heading", { name: "其中包含重要节点" })).toBeTruthy();
    expect(screen.getByText("共 1 个节点，含 1 个主动提交节点、1 个已改名节点")).toBeTruthy();
    expect(compactNodes).not.toHaveBeenCalled();

    // 第二次确认只提供滑动控件，点不开：必须在轨迹上拖到底才会执行。
    fireEvent.click(screen.getByRole("slider", { name: "拖动到底确认删除" }));
    expect(compactNodes).not.toHaveBeenCalled();

    dragSlideToEnd("拖动到底确认删除");
    await waitFor(() => expect(compactNodes).toHaveBeenCalledTimes(1));
  });

  it("deletes plain automatic backups after a single confirmation", async () => {
    const compactNodes = vi.fn().mockResolvedValue(true);
    controller.useHistoryController.mockReturnValue(controllerReturn({
      compactNodes,
      history: automaticHistory,
    }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "精简当前分支" }));
    // 「自动备份」同时出现在分支设置的分组标题上，这里只取历史节点卡片。
    const autoCard = screen.getAllByText("自动备份")
      .find((element) => element.closest(".history-node-card"));
    fireEvent.click(autoCard!.closest("button")!);
    fireEvent.click(screen.getByRole("button", { name: "删除节点" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除" }));

    await waitFor(() => expect(compactNodes).toHaveBeenCalledTimes(1));
    // 全部是未改名的自动备份节点：不再出现第二次确认的滑动控件。
    expect(screen.queryByRole("slider", { name: "拖动到底确认删除" })).toBeNull();
  });

  it("drops nodes that no longer exist from the compact selection", () => {
    const historyModule = () => <HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />;
    controller.useHistoryController.mockReturnValue(controllerReturn());

    const view = render(historyModule());
    fireEvent.click(screen.getByRole("button", { name: "精简当前分支" }));
    fireEvent.click(screen.getByText("Middle").closest("button")!);
    expect(screen.getByText("1 个已选")).toBeTruthy();

    // 精简中途被取消后这个节点已经不在树里：计数不能继续把它算在内。
    controller.useHistoryController.mockReturnValue(controllerReturn({
      history: { ...history, nodes: history.nodes.filter((item) => item.id !== "middle") },
    }));
    view.rerender(historyModule());

    expect(screen.getByText("0 个已选")).toBeTruthy();
  });

  it("runs the confirmation dialog's cancel against the operation while it is busy", async () => {
    const cancelOperation = vi.fn();
    // 精简挂起不返回：弹窗停在运行态，正是用户会去点取消的那一刻。
    const compactNodes = vi.fn(() => new Promise<boolean>(() => {}));
    const historyModule = () => <HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />;
    controller.useHistoryController.mockReturnValue(controllerReturn({
      compactNodes,
      cancelOperation,
      history: automaticHistory,
    }));

    const view = render(historyModule());
    fireEvent.click(screen.getByRole("button", { name: "精简当前分支" }));
    const autoCard = screen.getAllByText("自动备份")
      .find((element) => element.closest(".history-node-card"));
    fireEvent.click(autoCard!.closest("button")!);
    fireEvent.click(screen.getByRole("button", { name: "删除节点" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(compactNodes).toHaveBeenCalledTimes(1));

    // 操作已在运行：弹窗只剩「取消操作」这一个出口，而且它必须作用在操作上。
    controller.useHistoryController.mockReturnValue(controllerReturn({
      compactNodes,
      cancelOperation,
      history: automaticHistory,
      busy: true,
    }));
    view.rerender(historyModule());

    expect(screen.queryByRole("button", { name: "取消" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "取消操作" }));
    expect(cancelOperation).toHaveBeenCalledTimes(1);
  });

  it("re-queues a failed chain verification from the branch status row", () => {
    const reverifyBranch = vi.fn();
    const failedBranch = {
      ...branches[0],
      verifyError: "历史节点 ab12 完整性校验失败：snapshot 摘要与历史数据库不匹配",
      verifiedMs: null,
    };
    controller.useHistoryController.mockReturnValue(controllerReturn({
      reverifyBranch,
      history: { ...history, branches: [failedBranch, branches[1]] },
    }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    expect(screen.getByText("历史校验失败")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "查看历史校验失败详情" }));
    fireEvent.click(screen.getByRole("button", { name: "重新校验此分支" }));
    expect(reverifyBranch).toHaveBeenCalledWith("main");
  });
});

describe("HistoryModule node inspector", () => {
  beforeEach(() => {
    controller.useHistoryController.mockReturnValue(controllerReturn());
  });

  afterEach(() => cleanup());

  it("shows the selected node's ChunkFile format in branch view", async () => {
    const readNodeFormat = vi.fn().mockResolvedValue("v2");
    controller.useHistoryController.mockReturnValue(controllerReturn({ readNodeFormat }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    fireEvent.click(screen.getByRole("button", { name: /Main head/ }));

    expect(await screen.findByText("ChunkFile v2")).toBeTruthy();
    expect(readNodeFormat).toHaveBeenCalledWith("main-head");
  });

  it("shows the selected node's stored thumbnail in branch view", async () => {
    const readNodeThumbnail = vi.fn().mockResolvedValue("data:image/webp;base64,cHJldmlldw==");
    controller.useHistoryController.mockReturnValue(controllerReturn({ readNodeThumbnail }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    fireEvent.click(screen.getByRole("button", { name: /Main head/ }));

    const image = await screen.findByRole("img", { name: "Main head 的缩略图" });
    expect(image.getAttribute("src")).toBe("data:image/webp;base64,cHJldmlldw==");
    expect(readNodeThumbnail).toHaveBeenCalledWith("main-head");
  });

  it("shows a placeholder when the selected node has no thumbnail", async () => {
    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    fireEvent.click(screen.getByRole("button", { name: /Main head/ }));

    expect(await screen.findByText("无可用预览")).toBeTruthy();
  });
});
