import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BranchPublication, PublicationPreview, PublishResult } from "./types";
import {
  branches, deferred, outputPreview, previewImage, publication, publishedBranch, record,
} from "./testSupport";
import { usePublicationController } from "./usePublicationController";

const api = vi.hoisted(() => ({
  getPublication: vi.fn(),
  previewArtifact: vi.fn(),
  estimate: vi.fn(),
  previewPublication: vi.fn(),
  previewTile: vi.fn(),
  cancelOperation: vi.fn(),
  publish: vi.fn(),
  deleteRecord: vi.fn(),
  previewRecord: vi.fn(),
  exportRecord: vi.fn(),
  stageDroppedImage: vi.fn(),
  previewExternal: vi.fn(),
}));
const dialog = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn() }));

vi.mock("./api", () => ({ authenticityApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

const defaultOnError = vi.fn();

function options(selectedBranchId: string, onError = defaultOnError) {
  return {
    artworkTitle: "Artwork",
    branches,
    selectedBranchId,
    onError,
    onNavigateRecord: vi.fn(),
    onRetryFileCleanup: vi.fn().mockResolvedValue({ failures: [] }),
  };
}

describe("usePublicationController", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
    api.previewArtifact.mockResolvedValue(previewImage);
    api.estimate.mockResolvedValue({ jpegBytes: 7 });
    api.cancelOperation.mockResolvedValue(true);
    api.previewRecord.mockResolvedValue(previewImage);
  });

  afterEach(() => cleanup());

  it("keeps the latest branch when an older request resolves last", async () => {
    const first = deferred<BranchPublication>();
    const second = deferred<BranchPublication>();
    api.getPublication.mockImplementation((branchId: string) => branchId === "first" ? first.promise : second.promise);
    const onError = vi.fn();
    const { result, rerender } = renderHook(
      ({ selectedBranchId }) => usePublicationController(options(selectedBranchId, onError)),
      { initialProps: { selectedBranchId: "first" } },
    );

    rerender({ selectedBranchId: "second" });
    await act(async () => { second.resolve(publication("second")); });
    await waitFor(() => expect(result.current.publication?.branchId).toBe("second"));

    await act(async () => { first.resolve(publication("first")); });
    expect(result.current.publication?.branchId).toBe("second");
    expect(onError).not.toHaveBeenCalled();
  });

  it("keeps publication metadata available when the artifact preview fails and retries independently", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.previewArtifact
      .mockRejectedValueOnce(new Error("成品文件已损坏"))
      .mockResolvedValueOnce(previewImage);
    const onError = vi.fn();
    const { result } = renderHook(() => usePublicationController(options("first", onError)));

    await waitFor(() => expect(result.current.artifactPreviewError).toBe("成品文件已损坏"));
    expect(result.current.publication?.artifact?.branchId).toBe("first");
    expect(result.current.config?.branchId).toBe("first");
    expect(result.current.preview).toBeNull();
    expect(onError).not.toHaveBeenCalled();

    await act(async () => { await result.current.retryArtifactPreview(); });
    expect(result.current.preview).toEqual(previewImage);
    expect(result.current.artifactPreviewError).toBeNull();
  });

  it("contains an invalid size-estimate adapter result inside the debounced request", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.estimate.mockReturnValue(undefined);
    const { result } = renderHook(() => usePublicationController(options("first")));

    await waitFor(() => expect(result.current.publication?.branchId).toBe("first"));
    await waitFor(() => expect(api.estimate).toHaveBeenCalledOnce(), { timeout: 500 });
    expect(result.current.sizeEstimate).toBeNull();
  });

  it("clears an automatically generated TrustMark ID when switching branches", async () => {
    api.getPublication.mockImplementation((branchId: string) => Promise.resolve(publishedBranch(branchId)));
    api.previewPublication.mockResolvedValue(outputPreview("1".repeat(40)));
    const { result, rerender } = renderHook(
      ({ selectedBranchId }) => usePublicationController(options(selectedBranchId)),
      { initialProps: { selectedBranchId: "first" } },
    );
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => result.current.setPrivateKey("private-key"));
    await act(async () => { await result.current.generateOutputPreview(); });
    expect(result.current.watermarkId).toBe("1".repeat(40));

    rerender({ selectedBranchId: "second" });
    await waitFor(() => expect(result.current.publication?.branchId).toBe("second"));
    expect(result.current.watermarkId).toBe("");
  });

  it("clears the TrustMark ID when TrustMark is disabled", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    const { result } = renderHook(() => usePublicationController(options("first")));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => result.current.setWatermarkId("1".repeat(40)));
    act(() => result.current.setConfig((current) => current ? {
      ...current,
      trustmarkEnabled: false,
      additionalRegions: [],
    } : current));

    await waitFor(() => expect(result.current.watermarkId).toBe(""));
  });

  it("clears the TrustMark ID after a successful publication", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.previewPublication.mockResolvedValue(outputPreview("1".repeat(40)));
    api.publish.mockResolvedValue({
      record: record("first"),
      watermarkWarnings: [],
      renditionCacheHit: true,
      renderMs: 0,
      encodeMs: 0,
      signingMs: 25,
    });
    dialog.save.mockResolvedValue("C:/output.jpg");
    const { result } = renderHook(() => usePublicationController(options("first")));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => {
      result.current.setPrivateKey("private-key");
      result.current.setWatermarkId("1".repeat(40));
    });

    await act(async () => { await result.current.generateOutputPreview(); });
    await act(async () => { await result.current.publish(); });
    expect(api.publish).toHaveBeenCalledWith(expect.objectContaining({
      watermarkId: "1".repeat(40),
      previewCacheToken: "cache-token",
    }));
    expect(result.current.watermarkId).toBe("");
  });

  it("cancels an active quality preview without surfacing cancellation as an error", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    const pending = deferred<PublicationPreview>();
    api.previewPublication.mockReturnValue(pending.promise);
    const onError = vi.fn();
    const { result } = renderHook(() => usePublicationController(options("first", onError)));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => result.current.setPrivateKey("private-key"));

    let previewTask!: Promise<void>;
    act(() => { previewTask = result.current.generateOutputPreview(); });
    await waitFor(() => expect(result.current.outputPreviewBusy).toBe(true));
    await act(async () => { await result.current.cancelAuthenticityOperation(); });
    expect(api.cancelOperation).toHaveBeenCalledOnce();
    expect(result.current.cancelling).toBe(true);

    pending.reject(new Error("认证任务已取消"));
    await act(async () => { await previewTask; });
    expect(result.current.outputPreviewBusy).toBe(false);
    expect(result.current.cancelling).toBe(false);
    expect(onError).not.toHaveBeenCalledWith("认证任务已取消");
    expect(onError).toHaveBeenCalledWith("质量预览已取消。");
  });

  it("cancels an active publication without surfacing cancellation as an error", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    const pending = deferred<PublishResult>();
    api.publish.mockReturnValue(pending.promise);
    dialog.save.mockResolvedValue("C:/output.jpg");
    const onError = vi.fn();
    const { result } = renderHook(() => usePublicationController(options("first", onError)));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => result.current.setPrivateKey("private-key"));

    let publishTask!: Promise<void>;
    act(() => { publishTask = result.current.publish(); });
    await waitFor(() => expect(result.current.publishing).toBe(true));
    await act(async () => { await result.current.cancelAuthenticityOperation(); });
    expect(api.cancelOperation).toHaveBeenCalledOnce();
    expect(result.current.cancelling).toBe(true);

    pending.reject(new Error("认证任务已取消"));
    await act(async () => { await publishTask; });
    expect(result.current.publishing).toBe(false);
    expect(result.current.cancelling).toBe(false);
    expect(onError).not.toHaveBeenCalledWith("认证任务已取消");
    expect(onError).toHaveBeenCalledWith("签名发布已取消。");
  });

  it("deletes a record, leaves the view and refreshes the record list", async () => {
    const withRecord = (branchId: string): BranchPublication => ({
      ...publishedBranch(branchId),
      records: [record(branchId)],
    });
    api.getPublication.mockImplementation((branchId: string) => Promise.resolve(withRecord(branchId)));
    api.deleteRecord.mockResolvedValue({ failures: [] });
    const onError = vi.fn();
    const { result } = renderHook(() => usePublicationController(options("first", onError)));
    await waitFor(() => expect(result.current.publication?.records).toHaveLength(1));

    act(() => { result.current.openRecord(record("first")); });
    expect(result.current.viewingRecord?.id).toBe("record-1");

    await act(async () => { await result.current.deleteRecord(record("first")); });
    expect(api.deleteRecord).toHaveBeenCalledWith("record-1");
    expect(result.current.viewingRecord).toBeNull();
    expect(result.current.deletingRecord).toBe(false);
    expect(onError).not.toHaveBeenCalledWith(expect.anything());
    // load() 重新拉取发布状态以刷新记录列表。
    await waitFor(() => expect(api.getPublication.mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it("surfaces delete cleanup failures and reports the retry banner", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.deleteRecord.mockResolvedValue({
      failures: [{ id: "cleanup-1", path: "artworks/certified.jpg", error: "无法删除" }],
    });
    const onError = vi.fn();
    const { result } = renderHook(() => usePublicationController(options("first", onError)));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => { result.current.openRecord(record("first")); });

    await act(async () => { await result.current.deleteRecord(record("first")); });
    expect(result.current.cleanupFailures).toHaveLength(1);
    expect(onError).toHaveBeenCalledWith(expect.stringContaining("清理失败"));
  });

  it("reports record deletion errors without closing the view", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.deleteRecord.mockRejectedValue(new Error("找不到发布记录"));
    const onError = vi.fn();
    const { result } = renderHook(() => usePublicationController(options("first", onError)));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => { result.current.openRecord(record("first")); });

    await act(async () => { await result.current.deleteRecord(record("first")); });
    expect(onError).toHaveBeenCalledWith("找不到发布记录");
    expect(result.current.viewingRecord?.id).toBe("record-1");
    expect(result.current.deletingRecord).toBe(false);
  });

  it("exports a record to the chosen path", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.exportRecord.mockResolvedValue(undefined);
    dialog.save.mockResolvedValue("C:/exported.jpg");
    const onError = vi.fn();
    const { result } = renderHook(() => usePublicationController(options("first", onError)));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => { result.current.openRecord(record("first")); });

    await act(async () => { await result.current.exportRecord(record("first")); });
    expect(api.exportRecord).toHaveBeenCalledWith("record-1", "C:/exported.jpg");
    expect(result.current.exporting).toBe(false);
    expect(onError).toHaveBeenCalledWith(null);
  });

  it("reports export errors without closing the record view", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.exportRecord.mockRejectedValue(new Error("导出失败"));
    dialog.save.mockResolvedValue("C:/exported.jpg");
    const onError = vi.fn();
    const { result } = renderHook(() => usePublicationController(options("first", onError)));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));
    act(() => { result.current.openRecord(record("first")); });

    await act(async () => { await result.current.exportRecord(record("first")); });
    expect(onError).toHaveBeenCalledWith("导出失败");
    expect(result.current.viewingRecord?.id).toBe("record-1");
    expect(result.current.exporting).toBe(false);
  });

  it("selects the recorded branch once the branch list arrives", async () => {
    api.getPublication.mockResolvedValue(publication("second"));
    const { result, rerender } = renderHook(
      ({ list, selectedBranchId }) => usePublicationController({
        ...options(selectedBranchId),
        branches: list,
      }),
      { initialProps: { list: [] as typeof branches, selectedBranchId: "second" } },
    );

    // 分支列表尚未到达：记录的 branchId 暂时没有可匹配的分支。
    expect(result.current.selectedBranch).toBeNull();

    rerender({ list: branches, selectedBranchId: "second" });
    expect(result.current.selectedBranch?.id).toBe("second");
  });

  it("offers the branch artifact as the loupe source for framing", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.previewTile.mockResolvedValue(previewImage);
    const { result } = renderHook(() => usePublicationController(options("first")));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));

    await act(async () => {
      await result.current.previewLoupe({ x: 1, y: 2, width: 3, height: 4, maxEdge: 72 });
    });
    expect(api.previewTile).toHaveBeenCalledWith({
      source: "original",
      cacheToken: null,
      branchId: "first",
      x: 1,
      y: 2,
      width: 3,
      height: 4,
      maxEdge: 72,
    });
  });

  it("binds output-preview tiles to the current cache token and branch", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.previewPublication.mockResolvedValue(outputPreview("1".repeat(40)));
    api.previewTile.mockResolvedValue(previewImage);
    const { result } = renderHook(() => usePublicationController(options("first")));
    await waitFor(() => expect(result.current.preview).toEqual(previewImage));

    act(() => result.current.setPrivateKey("private-key"));
    await act(async () => { await result.current.generateOutputPreview(); });
    expect(result.current.outputPreview?.cacheToken).toBe("cache-token");

    const tile = { x: 1, y: 2, width: 3, height: 4, maxEdge: 72 };
    await act(async () => { await result.current.previewOutputTile("compressed", tile); });
    expect(api.previewTile).toHaveBeenCalledWith({
      source: "compressed",
      cacheToken: "cache-token",
      branchId: null,
      ...tile,
    });

    await act(async () => { await result.current.previewOutputTile("original", tile); });
    expect(api.previewTile).toHaveBeenCalledWith({
      source: "original",
      cacheToken: null,
      branchId: "first",
      ...tile,
    });
  });

  it("rejects compressed output-preview tiles before a preview exists", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    const { result } = renderHook(() => usePublicationController(options("first")));
    await waitFor(() => expect(result.current.publication?.branchId).toBe("first"));

    await expect(result.current.previewOutputTile("compressed", {
      x: 0, y: 0, width: 1, height: 1, maxEdge: 72,
    })).rejects.toThrow("导出预览尚未生成");
  });

  it("serves original output-preview tiles from the branch alone", async () => {
    api.getPublication.mockResolvedValue(publishedBranch("first"));
    api.previewTile.mockResolvedValue(previewImage);
    const { result } = renderHook(() => usePublicationController(options("first")));
    await waitFor(() => expect(result.current.publication?.branchId).toBe("first"));

    const tile = { x: 1, y: 2, width: 3, height: 4, maxEdge: 72 };
    await act(async () => { await result.current.previewOutputTile("original", tile); });
    expect(api.previewTile).toHaveBeenCalledWith({
      source: "original",
      cacheToken: null,
      branchId: "first",
      ...tile,
    });
  });
});
