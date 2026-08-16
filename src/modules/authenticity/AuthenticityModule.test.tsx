import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PublicationPreviewDialog } from "./AuthenticityModule";
import type { PublicationPreview } from "./types";

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

const preview: PublicationPreview = {
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
};

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
    vi.unstubAllGlobals();
  });

  it("preserves the numeric zoom while switching between the export and original images", async () => {
    render(<PublicationPreviewDialog preview={preview} busy={false} onBack={vi.fn()} onPublish={vi.fn()} />);

    fireEvent.click(screen.getByTitle("放大"));
    expect(screen.getByTitle("按预览像素显示").textContent).toBe("110%");

    fireEvent.click(screen.getByTitle("显示原图"));
    await waitFor(() => expect((screen.getByTitle("显示压缩预览") as HTMLButtonElement).disabled).toBe(false));
    expect(screen.getByTitle("按预览像素显示").textContent).toBe("110%");

    fireEvent.click(screen.getByTitle("显示压缩预览"));
    await waitFor(() => expect((screen.getByTitle("显示原图") as HTMLButtonElement).disabled).toBe(false));
    expect(screen.getByTitle("按预览像素显示").textContent).toBe("110%");
  });
});
