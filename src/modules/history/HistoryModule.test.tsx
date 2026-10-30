import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
    verifiedMs: null,
    verifyError: null,
    verifyIssue: null,
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
    lastCheckElapsedMs: null,
    lastCheckMode: null,
    lastError: null,
    consecutiveBackupFailures: 0,
    backupRetryAtMs: null,
    backupDisableNoticePending: false,
    finalArtifactLocked: false,
    publishedCount: 0,
    verifyError: null,
    verifyFailureCount: 0,
    verifyUnverifiableCount: 0,
    verifyIssue: null,
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
    lastCheckElapsedMs: null,
    lastCheckMode: null,
    lastError: null,
    consecutiveBackupFailures: 0,
    backupRetryAtMs: null,
    backupDisableNoticePending: false,
    finalArtifactLocked: false,
    publishedCount: 0,
    verifyError: null,
    verifyFailureCount: 0,
    verifyUnverifiableCount: 0,
    verifyIssue: null,
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
    resetArtworkHistory: vi.fn(),
    reverifyBranch: vi.fn(),
    deleteSubtree: vi.fn(),
    setCheckpoint: vi.fn(),
    forkBranch: vi.fn(),
    renameNode: vi.fn(),
    readNodeFormat: vi.fn().mockResolvedValue("v2"),
    readNodeThumbnail: vi.fn().mockResolvedValue({ dataUrl: null, supported: true }),
    refreshNodeThumbnail: vi.fn().mockResolvedValue(null),
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

  it("resets the whole artwork only after the menu, a click and the slide", async () => {
    const resetArtworkHistory = vi.fn().mockResolvedValue(true);
    controller.useHistoryController.mockReturnValue(controllerReturn({ resetArtworkHistory }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByTitle("更多历史操作"));
    fireEvent.click(screen.getByRole("button", { name: "重置作品历史" }));

    // 破坏性入口先收在二级菜单里；弹窗第一道只是普通确认，点「继续」也不会执行。
    expect(screen.getByRole("heading", { name: "重置作品历史" })).toBeTruthy();
    expect(screen.queryByRole("slider", { name: "拖动到底重置历史" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "继续" }));
    expect(resetArtworkHistory).not.toHaveBeenCalled();

    // 第二道必须把滑块拖到底。
    expect(screen.getByText("2 条分支 · 4 个历史节点将被永久删除")).toBeTruthy();
    dragSlideToEnd("拖动到底重置历史");
    await waitFor(() => expect(resetArtworkHistory).toHaveBeenCalledTimes(1));
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
      verifyFailureCount: 1,
      verifyUnverifiableCount: 0,
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

    expect(screen.getByText("1 个历史节点文件丢失或损坏")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "查看历史节点问题" }));
    fireEvent.click(screen.getByRole("button", { name: "重新检查此分支" }));
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

  it("tints unreadable nodes and explains what happened in the inspector", () => {
    const broken: ArtworkHistory = {
      ...history,
      nodes: history.nodes.map((node) => node.id === "main-head"
        ? {
          ...node,
          verifyIssue: "missing" as const,
          verifyError: "历史节点 main-head 完整性校验失败：文件缺失：恢复链起点 snapshot 无法打开（系统找不到指定的文件。 (os error 2)）",
        }
        : node),
    };
    controller.useHistoryController.mockReturnValue(controllerReturn({ history: broken }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    // 概览卡片整块染色，并在标签区直接说「文件丢失」，不再出现校验术语。
    const card = screen.getByRole("button", { name: /Main head/ });
    expect(card.classList.contains("verify-missing")).toBe(true);
    expect(within(card).getByText("文件丢失")).toBeTruthy();
    expect(card.getAttribute("title")).toContain("该节点文件丢失");
    expect(screen.getByRole("button", { name: /Middle/ }).classList.contains("verify-missing")).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    fireEvent.click(screen.getByRole("button", { name: /Main head/ }));

    // 节点信息栏给出同一结论，并把技术原文留在下面一行。
    expect(screen.getAllByText("文件丢失").length).toBeGreaterThan(1);
    expect(screen.getByText(/文件缺失：恢复链起点 snapshot 无法打开/)).toBeTruthy();
  });

  /// 被更深的坏节点挡住时既不能判坏也不能判好：灰底、虚线、「无法检查」，
  /// 并且说明里点明问题出在后续版本，而不是这个版本自己。
  it("marks a node that could not be checked as unknown instead of broken", () => {
    const blocked: ArtworkHistory = {
      ...history,
      nodes: history.nodes.map((node) => node.id === "middle"
        ? {
          ...node,
          verifyIssue: "unverifiable" as const,
          verifyError: "无法检查：历史节点 main-head 的备份文件（恢复链 delta）已丢失（系统找不到指定的文件。 (os error 2)），因此无法确认这个版本是否完好",
        }
        : node),
    };
    controller.useHistoryController.mockReturnValue(controllerReturn({ history: blocked }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    const card = screen.getByRole("button", { name: /Middle/ });
    expect(card.classList.contains("verify-unverifiable")).toBe(true);
    expect(card.classList.contains("verify-missing")).toBe(false);
    expect(within(card).getByText("无法检查")).toBeTruthy();
    expect(card.getAttribute("title")).toContain("后续版本的文件已丢失或损坏");

    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    // 切换视图会重建卡片，必须重新取一次。
    fireEvent.click(screen.getByRole("button", { name: /Middle/ }));

    // 信息栏与卡片同一套说法，并保留可以复制的原文。
    expect(document.querySelector(".history-verify-state.unverifiable")?.textContent).toBe("无法检查");
    expect(screen.getByText(/后续版本的文件已丢失或损坏，无法确认这个历史版本是否完好/)).toBeTruthy();
    expect(screen.getByText(/无法检查：历史节点 main-head/)).toBeTruthy();
  });

  it("shows the selected node's stored thumbnail in branch view", async () => {
    const readNodeThumbnail = vi.fn().mockResolvedValue({ dataUrl: "data:image/webp;base64,cHJldmlldw==", supported: true });
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
    expect(screen.getByRole("button", { name: "重新获取缩略图" })).toBeTruthy();
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

  it("re-extracts a missing thumbnail from the refresh button", async () => {
    const refreshNodeThumbnail = vi.fn().mockResolvedValue("data:image/webp;base64,cmVmcmVzaA==");
    controller.useHistoryController.mockReturnValue(controllerReturn({ refreshNodeThumbnail }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    fireEvent.click(screen.getByRole("button", { name: /Main head/ }));

    fireEvent.click(await screen.findByRole("button", { name: "重新获取缩略图" }));

    const image = await screen.findByRole("img", { name: "Main head 的缩略图" });
    expect(image.getAttribute("src")).toBe("data:image/webp;base64,cmVmcmVzaA==");
    expect(refreshNodeThumbnail).toHaveBeenCalledWith("main-head");
  });

  it("keeps the new preview when a refresh lands after the selection moved on", async () => {
    let finishRefresh: ((value: string | null) => void) | undefined;
    const refreshNodeThumbnail = vi.fn().mockImplementation(() => new Promise<string | null>((resolve) => {
      finishRefresh = resolve;
    }));
    const readNodeThumbnail = vi.fn().mockImplementation((id: string) => Promise.resolve(
      id === "middle"
        ? { dataUrl: "data:image/webp;base64,bWlkZGxl", supported: true }
        : { dataUrl: null, supported: true },
    ));
    controller.useHistoryController.mockReturnValue(controllerReturn({ readNodeThumbnail, refreshNodeThumbnail }));

    render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    fireEvent.click(screen.getByRole("button", { name: /Middle/ }));
    const middleImage = await screen.findByRole("img", { name: "Middle 的缩略图" });
    expect(middleImage.getAttribute("src")).toBe("data:image/webp;base64,bWlkZGxl");

    fireEvent.click(screen.getByRole("button", { name: "重新获取缩略图" }));
    fireEvent.click(screen.getByRole("button", { name: /Main head/ }));
    await screen.findByText("无可用预览");

    await act(async () => { finishRefresh?.("data:image/webp;base64,c3RhbGU="); });

    // The stale refresh belongs to "Middle"; the inspector must stay on the
    // node the user actually selected instead of showing its loading fallback.
    expect(screen.queryByText("读取预览…")).toBeNull();
    expect(screen.queryByRole("img", { name: "Middle 的缩略图" })).toBeNull();
    expect(screen.getByText("无可用预览")).toBeTruthy();
  });

  it("hides the preview block entirely when the format has no extractor", async () => {
    const readNodeThumbnail = vi.fn().mockResolvedValue({ dataUrl: null, supported: false });
    controller.useHistoryController.mockReturnValue(controllerReturn({ readNodeThumbnail }));

    const { container } = render(<HistoryModule
      artworkId="artwork-1"
      selectedBranchId="main"
      onSelectBranch={vi.fn()}
      onHistoryChanged={vi.fn()}
      onError={vi.fn()}
    />);

    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    fireEvent.click(screen.getByRole("button", { name: /Main head/ }));

    await waitFor(() => expect(readNodeThumbnail).toHaveBeenCalledWith("main-head"));
    await act(async () => {});

    expect(container.querySelector(".history-preview")).toBeNull();
    expect(screen.queryByText("无可用预览")).toBeNull();
  });
});

describe("HistoryModule node hover preview", () => {
  beforeEach(() => {
    controller.useHistoryController.mockReturnValue(controllerReturn());
  });

  afterEach(() => cleanup());

  const renderModule = () => render(<HistoryModule
    artworkId="artwork-1"
    selectedBranchId="main"
    onSelectBranch={vi.fn()}
    onHistoryChanged={vi.fn()}
    onError={vi.fn()}
  />);

  it("previews a node on hover and re-extracts a missing preview", async () => {
    const readNodeThumbnail = vi.fn().mockResolvedValue({ dataUrl: null, supported: true });
    const refreshNodeThumbnail = vi.fn().mockResolvedValue("data:image/webp;base64,aG92ZXI=");
    controller.useHistoryController.mockReturnValue(controllerReturn({ readNodeThumbnail, refreshNodeThumbnail }));

    const { container } = renderModule();
    fireEvent.mouseEnter(screen.getByText("Middle").closest("button")!);

    const popover = await waitFor(() => {
      const element = container.querySelector<HTMLElement>(".history-hover-preview");
      expect(element).toBeTruthy();
      return element!;
    });
    expect(readNodeThumbnail).toHaveBeenCalledWith("middle");
    expect(popover.textContent).toContain("无可用预览");

    fireEvent.click(within(popover).getByRole("button", { name: "重新获取缩略图" }));

    await waitFor(() => expect(popover.querySelector("img")?.getAttribute("src"))
      .toBe("data:image/webp;base64,aG92ZXI="));
    expect(refreshNodeThumbnail).toHaveBeenCalledWith("middle");
  });

  it("leaves hover unchanged when the format has no extractor", async () => {
    const readNodeThumbnail = vi.fn().mockResolvedValue({ dataUrl: null, supported: false });
    controller.useHistoryController.mockReturnValue(controllerReturn({ readNodeThumbnail }));

    const { container } = renderModule();
    fireEvent.mouseEnter(screen.getByText("Middle").closest("button")!);

    await waitFor(() => expect(readNodeThumbnail).toHaveBeenCalledWith("middle"));
    await act(async () => {});

    expect(container.querySelector(".history-hover-preview")).toBeNull();
  });

  // 当前分支视图的「节点信息」栏本来就会显示选中节点的缩略图，浮层不再重复一遍。
  it("does not preview on hover in the branch view", async () => {
    const readNodeThumbnail = vi.fn().mockResolvedValue({ dataUrl: "data:image/webp;base64,bWlkZGxl", supported: true });
    controller.useHistoryController.mockReturnValue(controllerReturn({ readNodeThumbnail }));

    const { container } = renderModule();
    fireEvent.click(screen.getByRole("button", { name: "当前分支" }));
    fireEvent.mouseEnter(screen.getByRole("button", { name: /Middle/ }));
    await act(async () => {});

    expect(container.querySelector(".history-hover-preview")).toBeNull();
    expect(readNodeThumbnail).not.toHaveBeenCalled();
  });
});
