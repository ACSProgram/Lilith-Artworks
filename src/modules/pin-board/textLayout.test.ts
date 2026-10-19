import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  exportTextMaxDimension,
  measurePinBoardText,
  rasterizePinBoardText,
  rasterizePinBoardTextPng,
  requestedTextTextureDimension,
  textRgbaPayload,
  textTextureDimensions,
} from "./textLayout";
import type { PinBoardTextItem } from "./types";

const fillText = vi.fn();
const context = {
  font: "",
  fillStyle: "",
  textBaseline: "",
  textAlign: "",
  measureText: (line: string) => ({ width: line.length * 10 }),
  clearRect: vi.fn(),
  fillText,
  getImageData: (_x: number, _y: number, width: number, height: number) => ({
    data: new Uint8ClampedArray(width * height * 4),
  }),
};

function textItem(overrides: Partial<PinBoardTextItem> = {}): PinBoardTextItem {
  return {
    kind: "text",
    boardId: 1,
    itemId: 7,
    width: 100,
    height: 50,
    order: 0,
    layer: 1,
    deleted: false,
    points: [[0, 0], [100, 0], [100, 50], [0, 50]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
    text: "ab\nc",
    fontFamily: '"Microsoft YaHei UI", "Segoe UI", sans-serif',
    fontSize: 20,
    color: "#112233",
    align: "center",
    ...overrides,
  };
}

beforeEach(() => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext")
    .mockReturnValue(context as unknown as CanvasRenderingContext2D);
  fillText.mockClear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("pin-board text layout", () => {
  it("measures the longest line and keeps a fixed line height", () => {
    expect(measurePinBoardText("ab\nc", 20)).toEqual({
      lines: ["ab", "c"],
      width: 20,
      height: 52,
      lineHeight: 26,
    });
  });

  it("scales raster dimensions by the requested long edge", () => {
    const item = textItem();
    expect(textTextureDimensions(item, 500)).toEqual([500, 250]);
    expect(requestedTextTextureDimension(item, 1000, 8192)).toBe(1250);
    expect(exportTextMaxDimension(item)).toBe(200);
  });

  it("emits the shared RGBA header and draws each line", () => {
    const item = textItem();
    const payload = textRgbaPayload(item, 200);
    expect(String.fromCharCode(...payload.subarray(0, 4))).toBe("RGBA");
    const view = new DataView(payload.buffer);
    expect(view.getUint32(4, true)).toBe(200);
    expect(view.getUint32(8, true)).toBe(100);
    expect(payload.byteLength).toBe(12 + 200 * 100 * 4);
    expect(fillText).toHaveBeenCalledTimes(2);
    expect(fillText.mock.calls[0][0]).toBe("ab");
    expect(fillText.mock.calls[1][0]).toBe("c");
  });

  it("rasters to the requested long edge", () => {
    const raster = rasterizePinBoardText(textItem(), 400);
    expect([raster.width, raster.height]).toEqual([400, 200]);
    expect(raster.rgba.byteLength).toBe(400 * 200 * 4);
  });

  it("encodes the export raster as PNG bytes", async () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);
    const toBlob = vi.fn((callback: BlobCallback, type?: string) => {
      expect(type).toBe("image/png");
      callback({ arrayBuffer: async () => png.buffer } as unknown as Blob);
    });
    vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation(toBlob);

    expect(await rasterizePinBoardTextPng(textItem(), 200)).toEqual(png);
    expect(toBlob).toHaveBeenCalledTimes(1);
    expect(fillText).toHaveBeenCalledTimes(2);
  });

  it("rejects when the canvas cannot encode a blob", async () => {
    vi.spyOn(HTMLCanvasElement.prototype, "toBlob")
      .mockImplementation((callback: BlobCallback) => callback(null));

    await expect(rasterizePinBoardTextPng(textItem(), 200)).rejects.toThrow("无法生成文字图片");
  });
});
