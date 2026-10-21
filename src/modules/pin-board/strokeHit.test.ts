import { describe, expect, it } from "vitest";
import {
  STROKE_TOOL,
  decodeStrokeHeader,
  type PinBoardStrokeGeometry,
  type PinBoardStrokeTool,
} from "./strokeGeometry";
import { arrowHeadPoints, encodeStrokeGeometry } from "./strokeRender";
import {
  buildStrokeWorldOutlines,
  strokeHitBounds,
  strokeHitByCircle,
  strokeHitBySweep,
  strokeIntersectsBounds,
  strokeOutlineSignature,
} from "./strokeHit";
import type { PinBoardStrokeItem } from "./types";

/** 100 × 20 的笔迹条目：四角就是世界坐标，便于把「局部」与「世界」对齐着断言。 */
function strokeItem(overrides: Partial<PinBoardStrokeItem> = {}): PinBoardStrokeItem {
  return {
    kind: "stroke",
    boardId: 1,
    itemId: 5,
    width: 100,
    height: 20,
    order: 0,
    layer: 3,
    deleted: false,
    points: [[0, 0], [100, 0], [100, 20], [0, 20]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
    tool: STROKE_TOOL.pen,
    color: "#e5484d",
    strokeWidth: 10,
    ...overrides,
  };
}

/** 造一条几何：头部字段直接从编码结果里解回来，不在这里重抄一遍头部口径。 */
function geometryFor(
  tool: PinBoardStrokeTool,
  samples: readonly { u: number; v: number; pressure?: number }[] = [],
): PinBoardStrokeGeometry {
  const geometry = encodeStrokeGeometry(tool, samples);
  return {
    itemId: 5,
    color: "#e5484d",
    strokeWidth: 10,
    width: 100,
    height: 20,
    ...decodeStrokeHeader(geometry),
    geometry,
  };
}

/** 水平方向的一条自由笔：局部 (10,10) → 世界 (10,10)，直到 (90,10)。 */
function horizontalPen() {
  return {
    item: strokeItem(),
    geometry: geometryFor(STROKE_TOOL.pen, [
      { u: 0.1, v: 0.5 },
      { u: 0.5, v: 0.5 },
      { u: 0.9, v: 0.5 },
    ]),
  };
}

describe("pin-board eraser hit testing", () => {
  it("hits a free stroke inside half the line width and misses outside it", () => {
    const { item, geometry } = horizontalPen();
    const outlines = buildStrokeWorldOutlines(item, geometry);
    // 线宽 10 → 半径 5：中心线上命中，偏 6 个单位就擦不到。
    expect(strokeHitByCircle(outlines, [50, 10], 0)).toBe(true);
    expect(strokeHitByCircle(outlines, [50, 16], 0)).toBe(false);
    // 擦头半径补上差额后又命中：判定用的是「墨迹半径 + 擦头半径」。
    expect(strokeHitByCircle(outlines, [50, 16], 2)).toBe(true);
  });

  it("uses the per-point pressure radius when the stroke has pressure", () => {
    const item = strokeItem({ strokeWidth: 10 });
    const geometry = geometryFor(STROKE_TOOL.pen, [
      { u: 0.1, v: 0.5, pressure: 0.2 },
      { u: 0.9, v: 0.5, pressure: 0.2 },
    ]);
    const outlines = buildStrokeWorldOutlines(item, geometry);
    // 半径 = 5 × (0.15 + 0.85 × 0.2) = 1.6，因此偏 3 个单位就擦不到。
    expect(strokeHitByCircle(outlines, [50, 13], 0)).toBe(false);
    expect(strokeHitByCircle(outlines, [50, 11.5], 0)).toBe(true);
  });

  it("follows a mirrored uv instead of the raw points", () => {
    // uv 左右镜像：局部 u=0.1 落到世界 x≈90 一侧，命中判定必须跟着镜像。
    // 采样点按 u16 编码（`encodeStrokeGeometry`），u=0.1 会有 ~1e-3 的量化误差，
    // 因此这里只断到世界坐标的百分位。
    const item = strokeItem({ uv: [[1, 0], [0, 0], [0, 1], [1, 1]] });
    const geometry = geometryFor(STROKE_TOOL.pen, [
      { u: 0.1, v: 0.5 },
      { u: 0.2, v: 0.5 },
    ]);
    const outlines = buildStrokeWorldOutlines(item, geometry);
    expect(outlines[0].points[0][0]).toBeCloseTo(90, 2);
    // 镜像后这一小段落在世界 x ≈ 80–90，另一侧（x ≈ 10）必须擦不到。
    expect(strokeHitByCircle(outlines, [90, 10], 0)).toBe(true);
    expect(strokeHitByCircle(outlines, [10, 10], 0)).toBe(false);
  });

  it("treats the four shapes as their drawn borders", () => {
    const line = buildStrokeWorldOutlines(
      strokeItem(),
      geometryFor(STROKE_TOOL.line),
    );
    // 方向位 0：从左上到右下，两端各内缩半个线宽。
    expect(strokeHitByCircle(line, [50, 10], 0)).toBe(true);
    expect(strokeHitByCircle(line, [50, 30], 0)).toBe(false);

    // 形状按局部宽高铺满四角，这里用 100 × 100 的方块，世界坐标才与局部坐标一一对应。
    const square: Partial<PinBoardStrokeItem> = {
      width: 100,
      height: 100,
      points: [[0, 0], [100, 0], [100, 100], [0, 100]],
    };
    const rect = buildStrokeWorldOutlines(
      strokeItem({ ...square, strokeWidth: 4 }),
      geometryFor(STROKE_TOOL.rect),
    );
    expect(strokeHitByCircle(rect, [50, 2], 0)).toBe(true);
    // 框内是空的：擦头必须碰到描边本身。
    expect(strokeHitByCircle(rect, [50, 50], 0)).toBe(false);

    const ellipse = buildStrokeWorldOutlines(
      strokeItem({ ...square, strokeWidth: 4 }),
      geometryFor(STROKE_TOOL.ellipse),
    );
    expect(strokeHitByCircle(ellipse, [98, 50], 1)).toBe(true);
    expect(strokeHitByCircle(ellipse, [50, 50], 0)).toBe(false);
  });

  it("includes the arrow head so the wings can be erased", () => {
    const item = strokeItem({ strokeWidth: 4 });
    const outlines = buildStrokeWorldOutlines(item, geometryFor(STROKE_TOOL.arrow));
    // 杆之外还有一块闭合的三角（头），从侧面擦必须是「擦得到」。
    expect(outlines).toHaveLength(2);
    expect(outlines[1].closed).toBe(true);
    const head = arrowHeadPoints([2, 2], [98, 18], 4);
    expect(head).not.toBeNull();
    const wing = head!.left;
    expect(strokeHitByCircle(outlines, wing, 0)).toBe(true);
    // 远离杆与头的空白处仍不命中。
    expect(strokeHitByCircle(outlines, [50, 60], 0)).toBe(false);
  });

  it("erases what the swept path crosses, not just what the pointer lands on", () => {
    const { item, geometry } = horizontalPen();
    const outlines = buildStrokeWorldOutlines(item, geometry);

    // 两个采样点都在笔迹之外（离中心线 60 与 40，半径 5），但连线垂直穿过它。
    expect(strokeHitBySweep(outlines, [[50, -50], [50, 50]], 0)).toBe(true);
    // 同一侧的两点：整段都离墨迹很远。
    expect(strokeHitBySweep(outlines, [[50, 40], [50, 60]], 0)).toBe(false);
    // 斜穿也算：两端各在笔迹一侧。
    expect(strokeHitBySweep(outlines, [[0, 40], [100, -20]], 0)).toBe(true);
    // 单个点退化为圆形判定。
    expect(strokeHitBySweep(outlines, [[50, 12]], 0)).toBe(true);
    expect(strokeHitBySweep(outlines, [[50, 40]], 0)).toBe(false);
    expect(strokeHitBySweep(outlines, [], 0)).toBe(false);
  });

  it("prefilters with the ink box expanded by the eraser radius", () => {
    const item = strokeItem();
    const bounds = strokeHitBounds(item, 6);
    expect(bounds).toEqual({ minX: -6, minY: -6, maxX: 106, maxY: 26 });
    expect(strokeHitBounds(item, 0)).toEqual({ minX: 0, minY: 0, maxX: 100, maxY: 20 });
  });

  it("invalidates the outline cache when the transform changes", () => {
    const item = strokeItem();
    const moved = strokeItem({ points: [[10, 0], [110, 0], [110, 20], [10, 20]] });
    expect(strokeOutlineSignature(item, 1)).not.toBe(strokeOutlineSignature(moved, 1));
    expect(strokeOutlineSignature(item, 1)).not.toBe(strokeOutlineSignature(item, 2));
    expect(strokeOutlineSignature(item, 1)).toBe(strokeOutlineSignature(strokeItem(), 1));
  });
});

describe("pin-board ink marquee hit testing", () => {
  /** 100 × 100、线宽 4 的方块：描边占 0–4 与 96–100 两条带。 */
  function squareRect() {
    const item = strokeItem({
      width: 100,
      height: 100,
      strokeWidth: 4,
      points: [[0, 0], [100, 0], [100, 100], [0, 100]],
    });
    return buildStrokeWorldOutlines(item, geometryFor(STROKE_TOOL.rect));
  }

  it("does not select a shape when the box only covers its hollow interior", () => {
    const outlines = squareRect();
    // 框罩住内部但不碰描边：这就是「按墨迹判」与「按包围框判」的分界。
    expect(strokeIntersectsBounds(outlines, { minX: 20, minY: 20, maxX: 80, maxY: 80 })).toBe(false);
    // 碰到左边那条描边就命中。
    expect(strokeIntersectsBounds(outlines, { minX: 0, minY: 20, maxX: 2, maxY: 80 })).toBe(true);
    // 完全罩住整个条目也是命中。
    expect(strokeIntersectsBounds(outlines, { minX: -10, minY: -10, maxX: 110, maxY: 110 })).toBe(true);
  });

  it("expands each segment by the ink radius and the given tolerance", () => {
    const outlines = squareRect();
    // 描边带宽 4（x ∈ 0–4）。内部窄条既碰不到墨迹，也碰不到 3 个单位容差之外的空白。
    expect(strokeIntersectsBounds(outlines, { minX: 20, minY: 20, maxX: 80, maxY: 80 }, 3)).toBe(false);
    // 离描边内侧 1 个单位：零容差不算命中，3 个单位容差算。
    const nearEdge = { minX: 5, minY: 20, maxX: 6, maxY: 80 };
    expect(strokeIntersectsBounds(outlines, nearEdge, 0)).toBe(false);
    expect(strokeIntersectsBounds(outlines, nearEdge, 3)).toBe(true);
  });

  it("detects a diagonal pen crossing the box without an endpoint inside", () => {
    const item = strokeItem({
      width: 100,
      height: 100,
      strokeWidth: 2,
      points: [[0, 0], [100, 0], [100, 100], [0, 100]],
    });
    const outlines = buildStrokeWorldOutlines(
      item,
      geometryFor(STROKE_TOOL.pen, [
        { u: 0, v: 0 },
        { u: 1, v: 1 },
      ]),
    );
    // 对角线两侧端点都在框外，只有中间穿过：靠四次边求交才判得出来。
    expect(strokeIntersectsBounds(outlines, { minX: 40, minY: 40, maxX: 60, maxY: 45 })).toBe(true);
    // 远离对角线的一角不命中。
    expect(strokeIntersectsBounds(outlines, { minX: 70, minY: 10, maxX: 90, maxY: 20 })).toBe(false);
  });

  it("treats a fully enclosed line segment as a hit", () => {
    const outlines = buildStrokeWorldOutlines(
      strokeItem({ width: 100, height: 20, strokeWidth: 2 }),
      geometryFor(STROKE_TOOL.pen, [
        { u: 0.4, v: 0.5 },
        { u: 0.6, v: 0.5 },
      ]),
    );
    // 整段落在框内。
    expect(strokeIntersectsBounds(outlines, { minX: 30, minY: 0, maxX: 70, maxY: 20 })).toBe(true);
    // 空折线不命中。
    expect(strokeIntersectsBounds([], { minX: 0, minY: 0, maxX: 100, maxY: 100 })).toBe(false);
  });
});
