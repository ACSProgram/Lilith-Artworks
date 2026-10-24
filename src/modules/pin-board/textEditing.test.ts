import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PinBoardTextItem } from "./types";
import {
  buildTextQuad,
  databaseFontSizeFor,
  effectivePinBoardFontSize,
  pinBoardTextCaretIndexAtPoint,
  pinBoardTextEditGeometry,
  pinBoardTextScale,
  pinBoardTextScreenMetrics,
  pinBoardTextSignature,
  resolveNewPinBoardTextDraft,
  resolvePinBoardTextDraft,
  textDraftFromItem,
  textQuadAngle,
} from "./textEditing";

const context = {
  font: "",
  measureText: (line: string) => ({
    width: line.length * 10,
    actualBoundingBoxAscent: 8,
    actualBoundingBoxDescent: 2,
  }),
};

function textItem(overrides: Partial<PinBoardTextItem> = {}): PinBoardTextItem {
  return {
    kind: "text",
    boardId: 1,
    itemId: 7,
    width: 20,
    height: 26,
    order: 0,
    layer: 1,
    deleted: false,
    points: [[-10, 13], [10, 13], [10, -13], [-10, -13]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
    text: "ab",
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
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("pin-board text editing geometry", () => {
  it("derives the item scale from the quad and the measured size", () => {
    const item = textItem({
      width: 20,
      height: 26,
      points: [[0, 0], [40, 0], [40, 52], [0, 52]],
    });
    expect(pinBoardTextScale(item)).toEqual({ x: 2, y: 2 });
    // 有效字号是字体栏显示的口径：数据库字号 × 条目缩放。
    expect(effectivePinBoardFontSize(item.fontSize, pinBoardTextScale(item).x)).toBe(40);
    expect(databaseFontSizeFor(40, pinBoardTextScale(item).x)).toBe(20);
  });

  it("rebuilds a quad around the original center and rotation", () => {
    const angle = Math.PI / 6;
    const quad = buildTextQuad([100, 50], angle, 80, 40);
    expect(textQuadAngle(quad)).toBeCloseTo(angle, 10);
    const center = quad.reduce(([x, y], point) => [x + point[0] / 4, y + point[1] / 4], [0, 0]);
    expect(center[0]).toBeCloseTo(100, 10);
    expect(center[1]).toBeCloseTo(50, 10);
    expect(Math.hypot(quad[1][0] - quad[0][0], quad[1][1] - quad[0][1])).toBeCloseTo(80, 10);
    expect(Math.hypot(quad[3][0] - quad[0][0], quad[3][1] - quad[0][1])).toBeCloseTo(40, 10);
  });

  it("maps the edit box center, rotation and pixel ratio through the viewport", () => {
    const item = textItem({ points: [[-50, 25], [50, 25], [50, -25], [-50, -25]] });
    const geometry = pinBoardTextEditGeometry(
      item,
      { width: 200, height: 100 },
      { centerX: 0, centerY: 0, height: 100 },
    );
    expect(geometry.centerX).toBeCloseTo(100, 10);
    expect(geometry.centerY).toBeCloseTo(50, 10);
    // 世界 y 向上、屏幕 y 向下：世界正角度在屏幕上表现为负角度。
    expect(geometry.angle).toBeCloseTo(0, 10);
    expect(geometry.pixelsPerWorldUnit).toBeCloseTo(1, 10);
  });

  it("grows the edit box with the content at the same screen font size", () => {
    const short = pinBoardTextScreenMetrics({ text: "ab", fontSize: 20 }, 1, 2);
    const long = pinBoardTextScreenMetrics({ text: "abcd", fontSize: 20 }, 1, 2);
    expect(short.screenFontSize).toBe(40);
    expect(short.width).toBe(20);
    expect(long.width).toBe(40);
    expect(long.height).toBe(short.height);
  });
});

describe("pin-board text payload", () => {
  it("treats upper- and lower-case colors as the same saved value", () => {
    expect(pinBoardTextSignature(textItem({ color: "#AABBCC" })))
      .toBe(pinBoardTextSignature(textItem({ color: "#aabbcc" })));
  });

  it("includes content, size, align and measurement in the signature", () => {
    const base = pinBoardTextSignature(textItem());
    for (const item of [
      textItem({ text: "abc" }),
      textItem({ fontSize: 21 }),
      textItem({ align: "left" }),
      textItem({ width: 21 }),
      textItem({ height: 27 }),
    ]) {
      expect(pinBoardTextSignature(item)).not.toBe(base);
    }
  });

  it("builds a draft from the persisted item", () => {
    expect(textDraftFromItem(textItem({ color: "#AABBCC" }))).toEqual({
      text: "ab",
      fontSize: 20,
      color: "#aabbcc",
      align: "center",
    });
  });

  it("resolves a valid draft into new metrics and a scaled quad", () => {
    const item = textItem({
      points: [[0, 0], [40, 0], [40, 52], [0, 52]],
    });
    const result = resolvePinBoardTextDraft(
      { text: "abcd", fontSize: 20, color: "#AABBCC", align: "left" },
      pinBoardTextScale(item),
      [20, 26],
      0,
    );
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.width).toBe(40);
    expect(result.height).toBe(26);
    expect(result.color).toBe("#aabbcc");
    // 条目缩放保持 2 倍：新测量宽 40 → 四边形宽 80。
    expect(Math.hypot(
      result.points[1][0] - result.points[0][0],
      result.points[1][1] - result.points[0][1],
    )).toBeCloseTo(80, 10);
  });

  it("rejects empty, over-long, out-of-range and invalid drafts", () => {
    const scale = { x: 1, y: 1 };
    const cases = [
      { text: "   ", fontSize: 20, color: "#000000", align: "left" as const },
      { text: "字".repeat(5001), fontSize: 20, color: "#000000", align: "left" as const },
      { text: "ab", fontSize: 0, color: "#000000", align: "left" as const },
      { text: "ab", fontSize: 1001, color: "#000000", align: "left" as const },
      { text: "ab", fontSize: 20, color: "green", align: "left" as const },
      { text: "ab", fontSize: 20, color: "#000000", align: "justify" as never },
      { text: "x".repeat(820), fontSize: 20, color: "#000000", align: "left" as const },
    ];
    for (const draft of cases) {
      const result = resolvePinBoardTextDraft(draft, scale, [0, 0], 0);
      expect(result.ok).toBe(false);
    }
  });
});

describe("pin-board new text payload", () => {
  it("resolves the draft into a create request with item scale 1", () => {
    const result = resolveNewPinBoardTextDraft(
      { text: "abcd", fontSize: 20, color: "#AABBCC", align: "right" },
      [100, 50],
    );
    expect(result).toEqual({
      ok: true,
      request: {
        text: "abcd",
        fontSize: 20,
        color: "#aabbcc",
        align: "right",
        measuredWidth: 40,
        measuredHeight: 26,
        centerX: 100,
        centerY: 50,
      },
    });
  });

  it("keeps the validation errors of the shared draft resolver", () => {
    const result = resolveNewPinBoardTextDraft(
      { text: "   ", fontSize: 20, color: "#000000", align: "center" },
      [0, 0],
    );
    expect(result.ok).toBe(false);
  });
});

describe("pin-board text caret from a world point", () => {
  it("maps a click inside an axis-aligned quad to the nearest boundary", () => {
    const item = textItem({
      text: "ab",
      width: 20,
      height: 26,
      fontSize: 20,
      points: [[0, 0], [40, 0], [40, 52], [0, 52]],
    });
    expect(pinBoardTextCaretIndexAtPoint(item, [30, 0])).toBe(1);
    expect(pinBoardTextCaretIndexAtPoint(item, [0, 0])).toBe(0);
    expect(pinBoardTextCaretIndexAtPoint(item, [40, 0])).toBe(2);
  });

  it("follows a rotated quad", () => {
    const points = buildTextQuad([100, 50], Math.PI / 6, 40, 52);
    const item = textItem({ text: "ab", width: 20, height: 26, fontSize: 20, points });
    // 四边形的几何中心落在两个字符之间。
    expect(pinBoardTextCaretIndexAtPoint(item, [100, 50])).toBe(1);
  });

  it("returns null for a degenerate quad", () => {
    const item = textItem({
      text: "ab",
      width: 20,
      height: 26,
      points: [[0, 0], [0, 0], [0, 0], [0, 0]],
    });
    expect(pinBoardTextCaretIndexAtPoint(item, [0, 0])).toBeNull();
  });
});
