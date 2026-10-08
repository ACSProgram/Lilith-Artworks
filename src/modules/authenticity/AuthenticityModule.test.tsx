import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthenticityModule, PublicationPreviewDialog } from "./AuthenticityModule";
import type { BranchPublication, PublicationPreview } from "./types";

const api = vi.hoisted(() => ({
  getPublication: vi.fn(),
  previewArtifact: vi.fn(),
  estimate: vi.fn(),
  previewPublication: vi.fn(),
  previewTile: vi.fn(),
  cancelOperation: vi.fn(),
  previewRecord: vi.fn(),
  deleteRecord: vi.fn(),
  stageDroppedImage: vi.fn(),
  previewExternal: vi.fn(),
}));

vi.mock("./api", () => ({ authenticityApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

class ResizeObserverMock {
  observe() {}
  disconnect() {}
}

class PreloadedImageMock {
  decoding = "auto";
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;

  set src(_value: string) {
    queueMicrotask(() => this.onload?.());
  }

  decode() {
    return Promise.resolve();
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

const preview: PublicationPreview = {
  branchId: "branch-1",
  image: {
    dataUrl: "data:image/jpeg;base64,cHJldmlldw==",
    width: 1000,
    height: 800,
    sourceBytes: 7,
  },
  originalImage: {
    dataUrl: "data:image/png;base64,b3JpZ2luYWw=",
    width: 1000,
    height: 800,
    sourceBytes: 8,
  },
  sourceWidth: 4000,
  sourceHeight: 3200,
  outputBytes: 7,
  watermarkId: null,
  cacheToken: "cache-token",
  cacheHit: false,
  renderMs: 12,
  encodeMs: 8,
};

const brokenPreviewPublication: BranchPublication = {
  branchId: "branch-1",
  artifact: {
    id: "artifact-1",
    branchId: "branch-1",
    historyId: "history-1",
    sourcePath: "artworks/final.png",
    sourceSha256: "a".repeat(64),
    mediaType: "image/png",
    byteSize: 7,
    createdMs: 1,
  },
  config: {
    branchId: "branch-1",
    title: "Artwork",
    creator: "Creator",
    rightsStatement: "",
    authenticationContent: "",
    trustmarkEnabled: false,
    certificatePath: "",
    signingAlgorithm: "es256",
    timestampUrl: null,
    jpegQuality: 90,
    backgroundColor: "#ffffff",
    watermarkStrength: 1,
    additionalRegions: [],
    updatedMs: 0,
  },
  records: [],
  modelsReady: false,
  modelVariant: "Q/BCH_SUPER",
  encoderSha256: null,
  decoderSha256: null,
};

describe("PublicationPreviewDialog", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
    vi.stubGlobal("ResizeObserver", ResizeObserverMock);
    vi.stubGlobal("Image", PreloadedImageMock);
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    api.cancelOperation.mockResolvedValue(true);
    api.previewTile.mockResolvedValue(preview.image);
  });

  it("keeps cancellation reachable when the stored artifact preview is damaged", async () => {
    api.getPublication.mockResolvedValue(brokenPreviewPublication);
    api.previewArtifact.mockRejectedValue(new Error("成品文件已损坏"));
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 7 });
    render(<AuthenticityModule
      mode="publish"
      artworkTitle="Artwork"
      branches={[{ id: "branch-1", title: "Main", headHistoryId: "history-1" }]}
      selectedBranchId="branch-1"
      onSelectBranch={vi.fn()}
      onError={vi.fn()}
      onNavigateRecord={vi.fn()}
      onRetryFileCleanup={vi.fn().mockResolvedValue({ failures: [] })}
    />);

    await screen.findByText("分支已进入发布状态，成品预览暂不可用");
    fireEvent.click(screen.getByTitle("更多发布操作"));
    fireEvent.click(screen.getByRole("button", { name: "取消发布并删除本地数据" }));
    expect(screen.getByRole("alertdialog", { name: "删除本地发布数据" })).toBeTruthy();
  });

  it("shows preview generation separately from its cancel action", async () => {
    const pending = deferred<PublicationPreview>();
    const onError = vi.fn();
    api.getPublication.mockResolvedValue({
      ...brokenPreviewPublication,
      config: { ...brokenPreviewPublication.config, certificatePath: "C:/certificate.pem" },
    });
    api.previewArtifact.mockResolvedValue(preview.image);
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 8 });
    api.previewPublication.mockReturnValue(pending.promise);
    render(<AuthenticityModule
      mode="publish"
      artworkTitle="Artwork"
      branches={[{ id: "branch-1", title: "Main", headHistoryId: "history-1" }]}
      selectedBranchId="branch-1"
      onSelectBranch={vi.fn()}
      onError={onError}
      onNavigateRecord={vi.fn()}
      onRetryFileCleanup={vi.fn().mockResolvedValue({ failures: [] })}
    />);

    fireEvent.change(await screen.findByPlaceholderText("输入后仅在本次发布使用"), {
      target: { value: "private-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "生成质量预览" }));

    expect(await screen.findByText("正在生成质量预览")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    await waitFor(() => expect(api.cancelOperation).toHaveBeenCalledOnce());
    expect(screen.getByText("正在取消质量预览")).toBeTruthy();

    await act(async () => { pending.reject(new Error("认证任务已取消")); });
    await waitFor(() => expect(onError).toHaveBeenCalledWith("质量预览已取消。"));
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("preserves the numeric zoom while switching between the export and original images", async () => {
    render(<PublicationPreviewDialog preview={preview} busy={false} cancelling={false} onBack={vi.fn()} onCancel={vi.fn()} onPublish={vi.fn()} />);

    fireEvent.click(screen.getByTitle("放大"));
    expect(screen.getByTitle("按原始像素显示（100% 为 1:1）").textContent).toBe("110%");

    fireEvent.click(screen.getByTitle("显示原图"));
    await waitFor(() => expect((screen.getByTitle("显示压缩预览") as HTMLButtonElement).disabled).toBe(false));
    expect(screen.getByTitle("按原始像素显示（100% 为 1:1）").textContent).toBe("110%");

    fireEvent.click(screen.getByTitle("显示压缩预览"));
    await waitFor(() => expect((screen.getByTitle("显示原图") as HTMLButtonElement).disabled).toBe(false));
    expect(screen.getByTitle("按原始像素显示（100% 为 1:1）").textContent).toBe("110%");
  });

  it("positions the high-resolution tile with inline geometry", async () => {
    const width = vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(1200);
    const height = vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(900);
    try {
      render(<PublicationPreviewDialog preview={preview} busy={false} cancelling={false} onBack={vi.fn()} onCancel={vi.fn()} onPublish={vi.fn()} />);

      const tile = await waitFor(() => {
        const node = document.querySelector<HTMLImageElement>(".publication-preview-tile");
        expect(node).not.toBeNull();
        return node as HTMLImageElement;
      });
      // 局部图的定位必须走 style，而不是被当成 left/top/width/height 属性展开——否则
      // 它会退回静态位置、掉到基础图下方。
      expect(tile.getAttribute("left")).toBeNull();
      expect(tile.getAttribute("top")).toBeNull();
      expect(tile.style.left).not.toBe("");
      expect(tile.style.top).not.toBe("");
      expect(tile.style.width).not.toBe("");
      expect(tile.style.height).not.toBe("");
      expect(api.previewTile).toHaveBeenCalledWith(expect.objectContaining({
        source: "compressed",
        cacheToken: "cache-token",
      }));
    } finally {
      width.mockRestore();
      height.mockRestore();
    }
  });

  it("exposes cancellation while signing is active", () => {
    const onCancel = vi.fn();
    render(<PublicationPreviewDialog preview={preview} busy cancelling={false} onBack={vi.fn()} onCancel={onCancel} onPublish={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "取消签名" }));
    expect(onCancel).toHaveBeenCalledOnce();
    expect(screen.getByText("正在写入 C2PA；时间戳服务最长等待 30 秒。")).toBeTruthy();
  });

  it("deletes a record from the read-only view through a confirm dialog", async () => {
    const record = {
      id: "record-1",
      artworkId: "artwork-1",
      artworkTitle: "Artwork",
      branchId: "branch-1",
      branchTitle: "Main",
      historyId: "history-1",
      watermarkId: null,
      trustmarkEnabled: false,
      outputPath: "C:/published/output.jpg",
      outputSha256: "b".repeat(64),
      outputBytes: 7,
      title: "Certified artwork",
      creator: "Creator",
      rightsStatement: "",
      authenticationContent: "",
      additionalRegions: [],
      c2paManifestLabel: null,
      c2paManifestJson: null,
      validationState: "Valid",
      createdMs: 1,
    };
    const recordPublication: BranchPublication = {
      ...brokenPreviewPublication,
      records: [record],
    };
    api.getPublication.mockResolvedValue(recordPublication);
    api.previewArtifact.mockResolvedValue(preview.image);
    api.previewRecord.mockResolvedValue(preview.image);
    api.deleteRecord.mockResolvedValue({ cleanedCount: 1, pendingCount: 0, failures: [] });
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 7 });
    render(<AuthenticityModule
      mode="publish"
      artworkTitle="Artwork"
      branches={[{ id: "branch-1", title: "Main", headHistoryId: "history-1" }]}
      selectedBranchId="branch-1"
      onSelectBranch={vi.fn()}
      onError={vi.fn()}
      onNavigateRecord={vi.fn()}
      onRetryFileCleanup={vi.fn().mockResolvedValue({ failures: [] })}
    />);

    fireEvent.click(await screen.findByRole("button", { name: /Certified artwork/ }));
    expect(await screen.findByText("发布记录 · 只读")).toBeTruthy();

    fireEvent.click(screen.getByTitle("更多记录操作"));
    fireEvent.click(screen.getByRole("button", { name: "删除本记录" }));
    expect(screen.getByRole("alertdialog", { name: "删除发布记录" })).toBeTruthy();
    expect(screen.getByText(/认证 JPG 副本/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "确认删除记录" }));
    await waitFor(() => expect(api.deleteRecord).toHaveBeenCalledWith("record-1"));
    await waitFor(() => expect(screen.queryByText("发布记录 · 只读")).toBeNull());
  });

  it("imports an image dropped onto the identify preview", async () => {
    api.stageDroppedImage.mockResolvedValue("C:/tmp/staged.png");
    api.previewExternal.mockResolvedValue(preview.image);
    render(<AuthenticityModule
      mode="identify"
      artworkTitle="Artwork"
      branches={[]}
      selectedBranchId={null}
      onSelectBranch={vi.fn()}
      onError={vi.fn()}
      onNavigateRecord={vi.fn()}
      onRetryFileCleanup={vi.fn().mockResolvedValue({ failures: [] })}
    />);

    const dropZone = screen.getByText("选择待识别图片").closest("section");
    const file = new File([new Uint8Array([1, 2, 3])], "art.png", { type: "image/png" });
    fireEvent.drop(dropZone!, { dataTransfer: { files: [file], types: ["Files"] } });

    await waitFor(() => expect(api.stageDroppedImage).toHaveBeenCalledWith({
      fileName: "art.png",
      dataBase64: expect.any(String),
    }));
    await waitFor(() => expect(api.previewExternal).toHaveBeenCalledWith("C:/tmp/staged.png"));
  });
});
