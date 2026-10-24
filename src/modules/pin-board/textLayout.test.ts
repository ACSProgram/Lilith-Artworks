import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  measurePinBoardText,
  pinBoardTextCaretIndex,
  rasterizePinBoardText,
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
  measureText: (line: string) => ({
    width: line.length * 10,
    actualBoundingBoxAscent: 8,
    actualBoundingBoxDescent: 2,
  }),
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

  describe("caret position from a point", () => {
    // measureText 桩把每个字符量成 10 宽，行高 = ceil(fontSize × 1.3)。
    it("picks the nearest character boundary on the clicked line", () => {
      expect(pinBoardTextCaretIndex("abc", 10, "left", 30, 21, 0)).toBe(2);
      expect(pinBoardTextCaretIndex("abc", 10, "left", 30, 0, 0)).toBe(0);
      expect(pinBoardTextCaretIndex("abc", 10, "left", 30, 30, 0)).toBe(3);
    });

    it("adds the offset of preceding lines", () => {
      // 第二行从第 3 个 UTF-16 单元开始（"ab\n"）。
      expect(pinBoardTextCaretIndex("ab\ncd", 10, "left", 20, 0, 20)).toBe(3);
    });

    it("clamps a point outside the block to the nearest line", () => {
      expect(pinBoardTextCaretIndex("ab\ncd", 10, "left", 20, 0, 500)).toBe(3);
      expect(pinBoardTextCaretIndex("ab\ncd", 10, "left", 20, 0, -50)).toBe(0);
    });

    it("applies the alignment offset before matching prefixes", () => {
      // 居中时 "c" 这一行（宽 10）从 5 开始：点 6 应落在行首。
      expect(pinBoardTextCaretIndex("ab\nc", 10, "center", 20, 6, 20)).toBe(3);
    });
  });

  it("scales raster dimensions by the requested long edge", () => {
    const item = textItem();
    expect(textTextureDimensions(item, 500)).toEqual([500, 250]);
    expect(requestedTextTextureDimension(item, 1000, 8192)).toBe(1250);
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

  it("centers each line's ink inside its line box", () => {
    // 宽 200、高 100 的纹理对 "ab / c"：字号 20 放大到 1.923 倍，
    // 行高 26 → 50 px；墨迹高 10 px，上下各留 20 px，基线再上移墨迹上界 8 px。
    rasterizePinBoardText(textItem(), 200);
    expect(context.textBaseline).toBe("alphabetic");
    expect(fillText.mock.calls.map((call) => call[2])).toEqual([28, 78]);
  });

});
