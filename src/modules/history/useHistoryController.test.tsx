import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { APP_EVENT_VERSION, type AppEvent } from "../../shared/appEvents";
import {
  dispatchRuntimeEvent,
  resetRuntimeEventBridgeForTests,
} from "../../shared/useRuntimeSubscription";
import type { ArtworkBranch, ArtworkHistory, BackupRuntimeStatus, HistoryNode } from "./types";

const historyApi = vi.hoisted(() => ({
  get: vi.fn(),
  runtime: vi.fn(),
  compact: vi.fn(),
  cancel: vi.fn(),
}));

vi.mock("./api", () => ({ historyApi }));

import { useHistoryController } from "./useHistoryController";

const history: ArtworkHistory = {
  artworkId: "artwork-1",
  artworkTitle: "Artwork",
  branches: [],
  nodes: [],
};

const runtime = (completionRevision: number): BackupRuntimeStatus => ({
  busy: false,
  activeBranchId: null,
  taskKind: null,
  operation: null,
  progressLabel: null,
  progressCurrent: 0,
  progressTotal: 0,
  automaticScheduling: true,
  completionRevision,
});

const node = (id: string): HistoryNode => ({
  id,
  createdOnBranchId: "branch-1",
  parentId: null,
  title: id,
  note: "",
  commitKind: "automatic",
  isCheckpoint: false,
  createdMs: 1,
  logicalSize: 1,
  chunkFileSize: 1,
  sha256: "a".repeat(64),
  chunkCount: 1,
});

const branch = (lastSuccessMs: number): ArtworkBranch => ({
  id: "branch-1",
  title: "Main",
  sourcePath: "C:\\work\\artwork.psd",
  headHistoryId: "n1",
  createdFromHistoryId: null,
  backupEnabled: true,
  backupIntervalMinutes: 30,
  backupQuickEnabled: true,
  lastCheckMs: lastSuccessMs,
  lastSuccessMs,
  lastError: null,
  consecutiveBackupFailures: 0,
  backupRetryAtMs: null,
  backupDisableNoticePending: false,
  finalArtifactLocked: false,
  publishedCount: 0,
  verifyError: null,
  verifiedMs: null,
});

const historyAt = (lastSuccessMs: number, withSecondNode = false): ArtworkHistory => ({
  artworkId: "artwork-1",
  artworkTitle: "Artwork",
  branches: [branch(lastSuccessMs)],
  nodes: withSecondNode ? [node("n1"), node("n2")] : [node("n1")],
});

const flush = async () => {
  for (let index = 0; index < 8; index += 1) await Promise.resolve();
};

const onError = vi.fn();
const onHistoryChanged = vi.fn();
let latest: ReturnType<typeof useHistoryController> | null = null;

function Harness() {
  const controller = useHistoryController({
    artworkId: "artwork-1",
    refreshVersion: 0,
    onHistoryChanged,
    onError,
  });
  latest = controller;
  return <span>{controller.history?.artworkTitle ?? "loading"}</span>;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function readyController() {
  historyApi.get.mockResolvedValue(history);
  historyApi.runtime.mockResolvedValue(runtime(0));
  render(<Harness />);
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
  return latest as ReturnType<typeof useHistoryController>;
}

describe("useHistoryController runtime polling", () => {
  afterEach(() => {
    cleanup();
    resetRuntimeEventBridgeForTests();
    vi.useRealTimers();
    vi.clearAllMocks();
    latest = null;
  });

  it("reloads immediately when a history operation finishes", async () => {
    vi.useFakeTimers();
    let currentRuntime = runtime(0);
    historyApi.get.mockResolvedValue(history);
    historyApi.runtime.mockImplementation(async () => currentRuntime);

    render(<Harness />);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(historyApi.get).toHaveBeenCalledTimes(1);

    currentRuntime = runtime(1);
    await act(async () => {
      dispatchRuntimeEvent({
        version: APP_EVENT_VERSION,
        type: "operationFinished",
        outcome: "succeeded",
        domain: "history",
        cancellable: true,
        completionRevision: 1,
        timestampMs: 1,
      } as AppEvent);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(historyApi.get).toHaveBeenCalledTimes(2);
  });

  it("reloads after a short operation that was never observed busy", async () => {
    vi.useFakeTimers();
    let currentRuntime = runtime(0);
    historyApi.get.mockResolvedValue(history);
    historyApi.runtime.mockImplementation(async () => currentRuntime);

    render(<Harness />);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(screen.getByText("Artwork")).toBeTruthy();
    expect(historyApi.get).toHaveBeenCalledTimes(1);

    currentRuntime = runtime(1);
    await act(async () => {
      vi.advanceTimersByTime(3_000);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(historyApi.get).toHaveBeenCalledTimes(2);
  });
});

describe("useHistoryController background completion refresh", () => {
  afterEach(() => {
    cleanup();
    resetRuntimeEventBridgeForTests();
    vi.useRealTimers();
    vi.clearAllMocks();
    latest = null;
  });

  async function setup() {
    vi.useFakeTimers();
    let currentRuntime = runtime(0);
    let currentHistory = historyAt(1);
    historyApi.get.mockImplementation(async () => currentHistory);
    historyApi.runtime.mockImplementation(async () => currentRuntime);
    render(<Harness />);
    await act(async () => {
      await flush();
    });
    return {
      publishRuntime: (value: BackupRuntimeStatus) => { currentRuntime = value; },
      publishHistory: (value: ArtworkHistory) => { currentHistory = value; },
    };
  }

  it("merges only branches for a non-structural completion", async () => {
    const state = await setup();
    const nodesBefore = latest!.history!.nodes;

    state.publishRuntime(runtime(1));
    state.publishHistory(historyAt(2));
    await act(async () => {
      dispatchRuntimeEvent({
        version: APP_EVENT_VERSION,
        type: "operationFinished",
        outcome: "succeeded",
        domain: "backup",
        artworkId: "artwork-1",
        structureChanged: false,
        cancellable: false,
        completionRevision: 1,
        timestampMs: 10,
      } as AppEvent);
      await flush();
    });

    expect(historyApi.get).toHaveBeenCalledTimes(2);
    expect(latest!.history!.branches[0].lastSuccessMs).toBe(2);
    // 节点数组保持同一引用：mindmap 不重建，缩略图缓存也不作废。
    expect(latest!.history!.nodes).toBe(nodesBefore);
  });

  it("reloads the whole history when the completion created a node", async () => {
    const state = await setup();
    const nodesBefore = latest!.history!.nodes;

    state.publishRuntime(runtime(1));
    state.publishHistory(historyAt(2, true));
    await act(async () => {
      dispatchRuntimeEvent({
        version: APP_EVENT_VERSION,
        type: "operationFinished",
        outcome: "succeeded",
        domain: "backup",
        artworkId: "artwork-1",
        structureChanged: true,
        cancellable: false,
        completionRevision: 1,
        timestampMs: 10,
      } as AppEvent);
      await flush();
    });

    expect(historyApi.get).toHaveBeenCalledTimes(2);
    expect(latest!.history!.nodes).not.toBe(nodesBefore);
    expect(latest!.history!.nodes).toHaveLength(2);
  });

  it("ignores a non-structural completion belonging to another artwork", async () => {
    const state = await setup();

    state.publishRuntime(runtime(1));
    state.publishHistory(historyAt(2));
    await act(async () => {
      dispatchRuntimeEvent({
        version: APP_EVENT_VERSION,
        type: "operationFinished",
        outcome: "succeeded",
        domain: "backup",
        artworkId: "artwork-2",
        structureChanged: false,
        cancellable: false,
        completionRevision: 1,
        timestampMs: 10,
      } as AppEvent);
      await flush();
    });

    expect(historyApi.get).toHaveBeenCalledTimes(1);
  });
});

describe("useHistoryController compact cancellation", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
    latest = null;
  });

  it("stops the batch when the cancel request could not reach the running node", async () => {
    const running = deferred<void>();
    const controller = await readyController();
    historyApi.compact.mockImplementationOnce(() => running.promise);
    // 原生侧此刻判定为空闲（取消落在两条命令之间的空档）：命令不会以取消错误退出，
    // 只有批级标志能拦住后面的节点。
    historyApi.cancel.mockResolvedValue(false);

    let batch!: Promise<boolean>;
    await act(async () => {
      batch = controller.compactNodes([node("n1"), node("n2"), node("n3")]);
    });
    await act(async () => {
      await controller.cancelOperation();
    });
    await act(async () => {
      running.resolve();
      await batch;
    });

    expect(historyApi.compact).toHaveBeenCalledTimes(1);
    expect(await batch).toBe(false);
    expect(onError).toHaveBeenCalledWith("精简已取消，剩余 2 个节点未被删除。");
  });

  it("reports the remaining count when the running node exits with the cancel error", async () => {
    const running = deferred<void>();
    const controller = await readyController();
    historyApi.compact.mockImplementationOnce(() => running.promise);
    historyApi.cancel.mockResolvedValue(true);

    let batch!: Promise<boolean>;
    await act(async () => {
      batch = controller.compactNodes([node("n1"), node("n2"), node("n3")]);
    });
    await act(async () => {
      await controller.cancelOperation();
    });
    // 取消落在节点命令的检查点上：命令自己以「操作已取消」退出，被取消的节点没有
    // 被删除，因此从它算起三个节点都还在。
    await act(async () => {
      running.reject(new Error("操作已取消"));
      await batch;
    });

    expect(historyApi.compact).toHaveBeenCalledTimes(1);
    expect(await batch).toBe(false);
    expect(onError).not.toHaveBeenCalledWith("操作已取消");
    expect(onError).toHaveBeenCalledWith("精简已取消，剩余 3 个节点未被删除。");
  });

  it("reports a cancel request that arrived after the operation finished", async () => {
    const running = deferred<void>();
    const controller = await readyController();
    historyApi.compact.mockImplementationOnce(() => running.promise);
    historyApi.cancel.mockResolvedValue(true);

    let batch!: Promise<boolean>;
    await act(async () => {
      batch = controller.compactNodes([node("n1")]);
    });
    await act(async () => {
      await controller.cancelOperation();
    });
    await act(async () => {
      running.resolve();
      await batch;
    });

    expect(await batch).toBe(true);
    expect(onError).toHaveBeenCalledWith("取消请求未被接受，操作已在取消到达前完成。");
  });
});
