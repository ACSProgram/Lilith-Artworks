import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  BranchPublication, CertificationConfig, CertificationRecord, PreviewImage, PublicationPreview,
} from "./types";
import { useIdentificationController, usePublicationController } from "./useAuthenticityController";

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
  stageDroppedImage: vi.fn(),
  previewExternal: vi.fn(),
}));
const dialog = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn() }));

vi.mock("./api", () => ({ authenticityApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

const config = (branchId: string): CertificationConfig => ({
  branchId,
  title: branchId,
  creator: "Creator",
  rightsStatement: "",
  authenticationContent: "",
  trustmarkEnabled: false,
  certificatePath: "C:/certificate.pem",
  signingAlgorithm: "es256",
  timestampUrl: null,
  jpegQuality: 90,
  backgroundColor: "#ffffff",
  watermarkStrength: 1,
  additionalRegions: [],
  updatedMs: 0,
});

const publication = (branchId: string): BranchPublication => ({
  branchId,
  artifact: null,
  config: config(branchId),
  records: [],
  modelsReady: false,
  modelVariant: "Q/BCH_SUPER",
  encoderSha256: null,
  decoderSha256: null,
});

const previewImage: PreviewImage = {
  dataUrl: "data:image/png;base64,cHJldmlldw==",
  width: 1200,
  height: 800,
  sourceBytes: 7,
};

const publishedBranch = (branchId: string): BranchPublication => ({
  ...publication(branchId),
  artifact: {
    id: `artifact-${branchId}`,
    branchId,
    historyId: `head-${branchId}`,
    sourcePath: `artworks/${branchId}.png`,
    sourceSha256: "a".repeat(64),
    mediaType: "image/png",
    byteSize: 7,
    createdMs: 1,
  },
  config: {
    ...config(branchId),
    trustmarkEnabled: true,
    additionalRegions: [{ x: 0.1, y: 0.1, width: 0.5, height: 0.5 }],
  },
  modelsReady: true,
});

const outputPreview = (watermarkId: string): PublicationPreview => ({
  branchId: "first",
  image: previewImage,
  originalImage: previewImage,
  sourceWidth: previewImage.width,
  sourceHeight: previewImage.height,
  outputBytes: previewImage.sourceBytes,
  watermarkId,
  cacheToken: "cache-token",
  cacheHit: false,
  renderMs: 12,
  encodeMs: 8,
});

const record = (branchId: string): CertificationRecord => ({
  id: "record-1",
  artworkId: "artwork-1",
  artworkTitle: "Artwork",
  branchId,
  branchTitle: branchId,
  historyId: `head-${branchId}`,
  watermarkId: "1".repeat(40),
  trustmarkEnabled: true,
  outputPath: "C:/output.jpg",
  outputSha256: "b".repeat(64),
  outputBytes: 7,
  title: branchId,
  creator: "Creator",
  rightsStatement: "",
  authenticationContent: "",
  additionalRegions: [{ x: 0.1, y: 0.1, width: 0.5, height: 0.5 }],
  c2paManifestLabel: "manifest",
  c2paManifestJson: "{}",
  validationState: "Valid",
  createdMs: 1,
});

const branches = [
  { id: "first", title: "First", headHistoryId: "head-first" },
  { id: "second", title: "Second", headHistoryId: "head-second" },
];
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
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 7 });
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
      width: previewImage.width,
      height: previewImage.height,
      watermarkRegionCount: 1,
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
});

describe("useIdentificationController", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => cleanup());

  it("stages a dropped image before previewing it", async () => {
    api.stageDroppedImage.mockResolvedValue("C:/tmp/staged.png");
    api.previewExternal.mockResolvedValue(previewImage);
    const onError = vi.fn();
    const { result } = renderHook(() => useIdentificationController({ onError }));

    await act(async () => { await result.current.importDropped("dropped.png", "AAAA"); });

    expect(api.stageDroppedImage).toHaveBeenCalledWith({ fileName: "dropped.png", dataBase64: "AAAA" });
    expect(api.previewExternal).toHaveBeenCalledWith("C:/tmp/staged.png");
    expect(result.current.path).toBe("C:/tmp/staged.png");
    expect(result.current.preview).toEqual(previewImage);
    expect(result.current.busy).toBe(false);
    expect(onError).not.toHaveBeenCalledWith(expect.any(String));
  });

  it("reports staging failures without changing the current preview", async () => {
    api.stageDroppedImage.mockRejectedValue(new Error("拖入的图片数据无效"));
    const onError = vi.fn();
    const { result } = renderHook(() => useIdentificationController({ onError }));

    await act(async () => { await result.current.importDropped("dropped.png", "!!"); });

    expect(onError).toHaveBeenCalledWith("拖入的图片数据无效");
    expect(result.current.preview).toBeNull();
    expect(result.current.busy).toBe(false);
  });

  it("requests the loupe block from the current external image", async () => {
    api.stageDroppedImage.mockResolvedValue("C:/tmp/staged.png");
    api.previewExternal.mockResolvedValue(previewImage);
    api.previewTile.mockResolvedValue(previewImage);
    const { result } = renderHook(() => useIdentificationController({ onError: vi.fn() }));

    await act(async () => { await result.current.importDropped("dropped.png", "AAAA"); });
    await act(async () => {
      await result.current.previewLoupe({ x: 0, y: 0, width: 72, height: 72, maxEdge: 72 });
    });

    expect(api.previewTile).toHaveBeenCalledWith({
      source: "external",
      cacheToken: null,
      branchId: null,
      path: "C:/tmp/staged.png",
      x: 0,
      y: 0,
      width: 72,
      height: 72,
      maxEdge: 72,
    });
  });
});
