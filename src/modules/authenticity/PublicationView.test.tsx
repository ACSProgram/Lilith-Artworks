import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dragSlideToEnd } from "../../shared/slideToConfirmTesting";
import { PublicationView, type PublicationViewProps } from "./PublicationView";
import type { BranchPublication, PublicationPreview } from "./types";
import {
  PreloadedImageMock, ResizeObserverMock, brokenPreviewPublication, deferred,
  previewImage, publicationPreview,
} from "./testSupport";

const api = vi.hoisted(() => ({
  getPublication: vi.fn(),
  previewArtifact: vi.fn(),
  estimate: vi.fn(),
  previewPublication: vi.fn(),
  previewTile: vi.fn(),
  cancelOperation: vi.fn(),
  previewRecord: vi.fn(),
  deleteRecord: vi.fn(),
  cancelPublication: vi.fn(),
  stageDroppedImage: vi.fn(),
  previewExternal: vi.fn(),
}));

vi.mock("./api", () => ({ authenticityApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const mainBranch = [{ id: "branch-1", title: "Main", headHistoryId: "history-1" }];

function renderPublication(overrides: Partial<PublicationViewProps> = {}) {
  return render(<PublicationView
    artworkTitle="Artwork"
    branches={mainBranch}
    selectedBranchId="branch-1"
    onSelectBranch={vi.fn()}
    onError={vi.fn()}
    onNavigateRecord={vi.fn()}
    onRetryFileCleanup={vi.fn().mockResolvedValue({ failures: [] })}
    {...overrides}
  />);
}

describe("PublicationView", () => {
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
    api.previewTile.mockResolvedValue(previewImage);
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("keeps cancellation reachable when the stored artifact preview is damaged", async () => {
    api.getPublication.mockResolvedValue(brokenPreviewPublication);
    api.previewArtifact.mockRejectedValue(new Error("成品文件已损坏"));
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 7 });
    api.cancelPublication.mockResolvedValue({ failures: [] });
    renderPublication();

    await screen.findByText("分支已进入发布状态，成品预览暂不可用");
    fireEvent.click(screen.getByTitle("更多发布操作"));
    fireEvent.click(screen.getByRole("button", { name: "取消发布并删除本地数据" }));
    expect(screen.getByRole("alertdialog", { name: "删除本地发布数据" })).toBeTruthy();

    // 删除本地发布数据不可撤销：确认按钮换成滑动控件，必须拖到底。
    const slider = screen.getByRole("slider", { name: "拖动到底删除本地数据" });
    fireEvent.click(slider);
    expect(api.cancelPublication).not.toHaveBeenCalled();

    dragSlideToEnd("拖动到底删除本地数据");
    await waitFor(() => expect(api.cancelPublication).toHaveBeenCalledWith("branch-1"));
  });

  it("shows preview generation separately from its cancel action", async () => {
    const pending = deferred<PublicationPreview>();
    const onError = vi.fn();
    api.getPublication.mockResolvedValue({
      ...brokenPreviewPublication,
      config: { ...brokenPreviewPublication.config, certificatePath: "C:/certificate.pem" },
    });
    api.previewArtifact.mockResolvedValue(previewImage);
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 8 });
    api.previewPublication.mockReturnValue(pending.promise);
    renderPublication({ onError });

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
    api.previewArtifact.mockResolvedValue(previewImage);
    api.previewRecord.mockResolvedValue(previewImage);
    api.deleteRecord.mockResolvedValue({ cleanedCount: 1, pendingCount: 0, failures: [] });
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 7 });
    renderPublication();

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

  it("distinguishes loading, error and empty branch states on the publish view", async () => {
    const onRetryBranches = vi.fn();
    const base = {
      artworkTitle: "Artwork",
      selectedBranchId: null,
      onSelectBranch: vi.fn(),
      onError: vi.fn(),
      onNavigateRecord: vi.fn(),
      onRetryFileCleanup: vi.fn().mockResolvedValue({ failures: [] }),
    };
    const { rerender } = render(<PublicationView {...base} branches={[]} branchesLoading />);

    // 加载中：不再误报「尚无分支」，并且分支选择被禁用。
    expect(screen.getByText("读取分支历史…")).toBeTruthy();
    expect(screen.queryByText("此 Artwork 尚无分支。")).toBeNull();
    expect((screen.getByRole("combobox") as HTMLSelectElement).disabled).toBe(true);

    rerender(<PublicationView {...base} branches={[]} branchesError="无法读取作品历史" onRetryBranches={onRetryBranches} />);
    expect(screen.getByText("无法读取作品历史")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(onRetryBranches).toHaveBeenCalledOnce();

    // 加载完成且确实没有分支：保留原文案。
    rerender(<PublicationView {...base} branches={[]} />);
    expect(screen.getByText("此 Artwork 尚无分支。")).toBeTruthy();

    api.getPublication.mockResolvedValue({ ...brokenPreviewPublication, artifact: null });
    rerender(<PublicationView
      {...base}
      branches={mainBranch}
      selectedBranchId="branch-1"
    />);
    expect(await screen.findByText(/进入发布状态/)).toBeTruthy();
  });

  it("warns in place while framing a selection too small for pixel-domain readback", async () => {
    const smallRegion: BranchPublication = {
      ...brokenPreviewPublication,
      modelsReady: true,
      config: {
        ...brokenPreviewPublication.config,
        trustmarkEnabled: true,
        additionalRegions: [{ x: 0.1, y: 0.1, width: 0.1, height: 0.1 }],
      },
    };
    api.getPublication.mockResolvedValue(smallRegion);
    api.previewArtifact.mockResolvedValue(previewImage);
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 7 });
    renderPublication();

    // 1200x800 源图上的 0.1x0.1 选区只有 120x80 px，任一边小于 256：框选时就地提示。
    expect(await screen.findByText(/小于 256 px/)).toBeTruthy();
  });

  it("keeps the publication preview dialog wired to the controller", async () => {
    api.getPublication.mockResolvedValue({
      ...brokenPreviewPublication,
      config: { ...brokenPreviewPublication.config, certificatePath: "C:/certificate.pem" },
    });
    api.previewArtifact.mockResolvedValue(previewImage);
    api.estimate.mockResolvedValue({ jpegBytes: 7, sourceBytes: 8 });
    api.previewPublication.mockResolvedValue(publicationPreview);
    renderPublication();

    fireEvent.change(await screen.findByPlaceholderText("输入后仅在本次发布使用"), {
      target: { value: "private-key" },
    });
    fireEvent.click(screen.getByRole("button", { name: "生成质量预览" }));
    expect(await screen.findByRole("dialog", { name: "导出预览" })).toBeTruthy();
  });
});
