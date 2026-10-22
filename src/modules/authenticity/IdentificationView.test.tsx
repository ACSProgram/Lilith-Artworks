import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IdentificationView } from "./IdentificationView";
import { ResizeObserverMock, previewImage } from "./testSupport";

const api = vi.hoisted(() => ({
  stageDroppedImage: vi.fn(),
  previewExternal: vi.fn(),
  previewTile: vi.fn(),
}));

vi.mock("./api", () => ({ authenticityApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

describe("IdentificationView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal("ResizeObserver", ResizeObserverMock);
    api.previewTile.mockResolvedValue(previewImage);
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("imports an image dropped onto the identify preview", async () => {
    api.stageDroppedImage.mockResolvedValue("C:/tmp/staged.png");
    api.previewExternal.mockResolvedValue(previewImage);
    render(<IdentificationView onError={vi.fn()} onNavigateRecord={vi.fn()} />);

    const dropZone = screen.getByText("选择待识别图片").closest("section");
    const file = new File([new Uint8Array([1, 2, 3])], "art.png", { type: "image/png" });
    fireEvent.drop(dropZone!, { dataTransfer: { files: [file], types: ["Files"] } });

    await waitFor(() => expect(api.stageDroppedImage).toHaveBeenCalledWith({
      fileName: "art.png",
      dataBase64: expect.any(String),
    }));
    await waitFor(() => expect(api.previewExternal).toHaveBeenCalledWith("C:/tmp/staged.png"));
  });

  it("shows the source-resolution loupe while hovering the identify image", async () => {
    vi.stubGlobal("PointerEvent", window.MouseEvent);
    // jsdom 没有 2D 上下文；镜片绘制逻辑由 loupe 单测覆盖，这里只验证交互与请求。
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    api.stageDroppedImage.mockResolvedValue("C:/tmp/staged.png");
    api.previewExternal.mockResolvedValue(previewImage);
    render(<IdentificationView onError={vi.fn()} onNavigateRecord={vi.fn()} />);

    const dropZone = screen.getByText("选择待识别图片").closest("section");
    const file = new File([new Uint8Array([1, 2, 3])], "art.png", { type: "image/png" });
    fireEvent.drop(dropZone!, { dataTransfer: { files: [file], types: ["Files"] } });

    const layer = await waitFor(() => {
      const node = document.querySelector<HTMLElement>(".region-layer");
      expect(node).not.toBeNull();
      return node as HTMLElement;
    });
    // jsdom 没有布局，显式给出图片显示矩形，让归一化坐标可换算成源像素。
    layer.getBoundingClientRect = () => ({
      left: 0, top: 0, right: 200, bottom: 150, width: 200, height: 150, x: 0, y: 0,
      toJSON: () => ({}),
    }) as DOMRect;
    fireEvent.pointerMove(layer, { clientX: 100, clientY: 75 });

    // 镜片立即出现（底图先出画），源分辨率瓦片在后台补齐。
    await waitFor(() => expect(document.querySelector(".region-loupe")).not.toBeNull());
    await waitFor(() => expect(api.previewTile).toHaveBeenCalledWith(expect.objectContaining({
      source: "external",
      path: "C:/tmp/staged.png",
      width: 512,
      height: 512,
      maxEdge: 512,
    })));

    // 在瓦片到位前移动指针不会重新请求：同一格内移动只是重绘。
    const calls = api.previewTile.mock.calls.length;
    fireEvent.pointerMove(layer, { clientX: 104, clientY: 78 });
    await act(async () => { await Promise.resolve(); });
    expect(api.previewTile.mock.calls.length).toBe(calls);

    // React 由 pointerout/pointerover 合成 pointerenter/pointerleave，因此两个事件都派发。
    fireEvent.pointerOut(layer, { relatedTarget: document.body });
    fireEvent.pointerLeave(layer);
    expect(document.querySelector(".region-loupe")).toBeNull();
  });
});
