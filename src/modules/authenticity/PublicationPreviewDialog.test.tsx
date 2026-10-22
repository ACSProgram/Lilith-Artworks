import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PublicationPreviewDialog } from "./PublicationPreviewDialog";
import { PreloadedImageMock, ResizeObserverMock, publicationPreview } from "./testSupport";

describe("PublicationPreviewDialog", () => {
  beforeEach(() => {
    vi.stubGlobal("ResizeObserver", ResizeObserverMock);
    vi.stubGlobal("Image", PreloadedImageMock);
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      callback(0);
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("preserves the numeric zoom while switching between the export and original images", async () => {
    render(<PublicationPreviewDialog preview={publicationPreview} busy={false} cancelling={false} onRequestTile={async () => publicationPreview.image} onBack={vi.fn()} onCancel={vi.fn()} onPublish={vi.fn()} />);

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
    const onRequestTile = vi.fn(async () => publicationPreview.image);
    try {
      render(<PublicationPreviewDialog preview={publicationPreview} busy={false} cancelling={false} onRequestTile={onRequestTile} onBack={vi.fn()} onCancel={vi.fn()} onPublish={vi.fn()} />);

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
      expect(onRequestTile).toHaveBeenCalledWith("compressed", expect.objectContaining({
        x: expect.any(Number),
        y: expect.any(Number),
        width: expect.any(Number),
        height: expect.any(Number),
        maxEdge: expect.any(Number),
      }));
    } finally {
      width.mockRestore();
      height.mockRestore();
    }
  });

  it("exposes cancellation while signing is active", () => {
    const onCancel = vi.fn();
    render(<PublicationPreviewDialog preview={publicationPreview} busy cancelling={false} onRequestTile={async () => publicationPreview.image} onBack={vi.fn()} onCancel={onCancel} onPublish={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "取消签名" }));
    expect(onCancel).toHaveBeenCalledOnce();
    expect(screen.getByText("正在写入 C2PA；时间戳服务最长等待 30 秒。")).toBeTruthy();
  });
});
