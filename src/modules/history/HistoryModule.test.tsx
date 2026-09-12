import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HistoryModule } from "./HistoryModule";
import type { ArtworkBranch, ArtworkHistory, BackupRuntimeStatus, HistoryNode } from "./types";

const controller = vi.hoisted(() => ({ useHistoryController: vi.fn() }));
vi.mock("./useHistoryController", () => controller);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const idleRuntime: BackupRuntimeStatus = {
  busy: false,
  activeBranchId: null,
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
    lastCheckMs: null,
    lastSuccessMs: null,
    lastError: null,
    consecutiveBackupFailures: 0,
    backupRetryAtMs: null,
    backupDisableNoticePending: false,
    finalArtifactLocked: false,
    publishedCount: 0,
  },
  {
    id: "other",
    title: "Other",
    sourcePath: "C:\\work\\other.psd",
    headHistoryId: "other-head",
    createdFromHistoryId: "root",
    backupEnabled: true,
    backupIntervalMinutes: 10,
    lastCheckMs: null,
    lastSuccessMs: null,
    lastError: null,
    consecutiveBackupFailures: 0,
    backupRetryAtMs: null,
    backupDisableNoticePending: false,
    finalArtifactLocked: false,
    publishedCount: 0,
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

describe("HistoryModule overview interactions", () => {
  beforeEach(() => {
    controller.useHistoryController.mockReturnValue({
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
      deleteSubtree: vi.fn(),
      setCheckpoint: vi.fn(),
      forkBranch: vi.fn(),
      renameNode: vi.fn(),
      cancelOperation: vi.fn(),
    });
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
});
