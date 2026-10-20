import { describe, expect, it } from "vitest";
import { decodeStrokeHeader, STROKE_TOOL, type PinBoardStrokeGeometry } from "./strokeGeometry";
import {
  STROKE_OUTLINE_VERTICES,
  STROKE_PRIMITIVE_KIND,
  STROKE_VERTEX_FLOATS,
  buildStrokeBatch,
  buildStrokeOutlines,
  decodeStrokeSamples,
  encodeStrokeGeometry,
  mapLocalPoint,
  shapeAnchors,
  strokePointRadius,
  strokeWorldCorners,
  type StrokeBatch,
  type StrokeDrawEntry,
} from "./strokeRender";
import type { Point, Quad } from "./geometry";
import type { PinBoardStrokeItem } from "./types";

const IDENTITY_UV: Quad = [[0, 0], [1, 0], [1, 1], [0, 1]];
const BOX_POINTS: Quad = [[0, 0], [100, 0], [100, 100], [0, 100]];

function strokeItem(overrides: Partial<PinBoardStrokeItem> = {}): PinBoardStrokeItem {
  return {
    boardId: 1,
    itemId: 1,
    kind: "stroke",
    width: 100,
    height: 100,
    order: 1,
    layer: 3,
    deleted: false,
    points: BOX_POINTS,
    uv: IDENTITY_UV,
    available: true,
    tool: STROKE_TOOL.pen,
    color: "#FF8000",
    strokeWidth: 2,
    ...overrides,
  };
}

function geometryFor(
  tool: (typeof STROKE_TOOL)[keyof typeof STROKE_TOOL],
  samples: { u: number; v: number; pressure?: number }[] = [],
  direction = 0,
): PinBoardStrokeGeometry {
  const bytes = encodeStrokeGeometry(tool, samples, direction);
  return {
    ...decodeStrokeHeader(bytes),
    itemId: 1,
    color: "#FF8000",
    strokeWidth: 2,
    width: 100,
    height: 100,
    geometry: bytes,
  };
}

function entry(item: PinBoardStrokeItem, geometry: PinBoardStrokeGeometry): StrokeDrawEntry {
  return { item, geometry };
}

function vertex(batch: StrokeBatch, index: number): number[] {
  const start = index * STROKE_VERTEX_FLOATS;
  return Array.from(batch.vertices.subarray(start, start + STROKE_VERTEX_FLOATS));
}

function expectFinite(batch: StrokeBatch) {
  for (const value of batch.vertices) expect(Number.isFinite(value)).toBe(true);
}

describe("pin-board stroke geometry encoding", () => {
  it("round-trips pen samples through the BLOB", () => {
    const geometry = geometryFor(STROKE_TOOL.pen, [
      { u: 0, v: 0, pressure: 0 },
      { u: 0.5, v: 0.25, pressure: 0.5 },
      { u: 1, v: 1, pressure: 1 },
    ]);

    const samples = decodeStrokeSamples(geometry);

    expect(samples).toHaveLength(3);
    expect(samples[1].u).toBeCloseTo(0.5, 3);
    expect(samples[1].v).toBeCloseTo(0.25, 3);
    expect(samples[1].pressure).toBeCloseTo(0.5, 2);
    expect(geometry.hasPressure).toBe(true);
  });

  it("marks shapes as point-free and carries the direction bits", () => {
    const geometry = geometryFor(STROKE_TOOL.rect, [], 2);

    expect(geometry.isShape).toBe(true);
    expect(geometry.pointCount).toBe(0);
    expect(geometry.direction).toBe(2);
    expect(geometry.geometry).toHaveLength(8);
    expect(decodeStrokeSamples(geometry)).toEqual([]);
  });

  it("rejects a pen stroke without points and one over the point limit", () => {
    expect(() => encodeStrokeGeometry(STROKE_TOOL.pen, [])).toThrow();
    const tooMany = Array.from({ length: 4097 }, () => ({ u: 0, v: 0 }));
    expect(() => encodeStrokeGeometry(STROKE_TOOL.pen, tooMany)).toThrow();
  });
});

describe("pin-board stroke local mapping", () => {
  it("maps the four local corners onto the item quad", () => {
    expect(strokeWorldCorners(BOX_POINTS, IDENTITY_UV)).toEqual(BOX_POINTS);
  });

  it("follows a mirrored uv quad so a flipped stroke mirrors with the item", () => {
    const flipped: Quad = [IDENTITY_UV[1], IDENTITY_UV[0], IDENTITY_UV[3], IDENTITY_UV[2]];

    const corners = strokeWorldCorners(BOX_POINTS, flipped);

    // 局部 (0,0) 现在落在 points[1]（原图右上角），水平镜像。
    expect(corners[0]).toEqual([100, 0]);
    expect(corners[1]).toEqual([0, 0]);
    expect(corners[2]).toEqual([0, 100]);
    expect(corners[3]).toEqual([100, 100]);
  });

  it("maps the local center to the quad center", () => {
    const center = mapLocalPoint(BOX_POINTS, [50, 50], 100, 100);

    expect(center[0]).toBeCloseTo(50);
    expect(center[1]).toBeCloseTo(50);
  });

  it("keeps the center of a rotated quad at its geometric center", () => {
    // 绕原点旋转 45° 后的正方形（世界坐标）。
    const rotated: Quad = [[0, 0], [70.71, 70.71], [0, 141.42], [-70.71, 70.71]];

    const center = mapLocalPoint(rotated, [50, 50], 100, 100);

    expect(center[0]).toBeCloseTo(0, 2);
    expect(center[1]).toBeCloseTo(70.71, 2);
  });
});

describe("pin-board stroke primitives", () => {
  it("builds one tapered capsule per pen segment", () => {
    const item = strokeItem();
    const geometry = geometryFor(STROKE_TOOL.pen, [
      { u: 0, v: 0, pressure: 0 },
      { u: 1, v: 1, pressure: 0 },
    ]);

    const batch = buildStrokeBatch([entry(item, geometry)], { pixelsPerWorldUnit: 1 });

    expect(batch.primitiveCount).toBe(1);
    expect(batch.strokeCount).toBe(1);
    expect(batch.indices).toHaveLength(6);
    expect(batch.vertices).toHaveLength(4 * STROKE_VERTEX_FLOATS);
    // 四边形覆盖线段两端：四个世界顶点关于线段中点对称。
    const centroid = [0, 1, 2, 3].reduce(
      (sum, index) => {
        const values = vertex(batch, index);
        return [sum[0] + values[0] / 4, sum[1] + values[1] / 4];
      },
      [0, 0],
    );
    expect(centroid[0]).toBeCloseTo(50);
    expect(centroid[1]).toBeCloseTo(50);
    // 局部坐标落在包围盒内（外扩不超过「半径 + 一个像素羽化 + 端点圆头」的余量）。
    for (let index = 0; index < 4; index += 1) {
      const values = vertex(batch, index);
      expect(values[2]).toBeGreaterThanOrEqual(-5);
      expect(values[2]).toBeLessThanOrEqual(105);
      expect(values[3]).toBeGreaterThanOrEqual(-5);
      expect(values[3]).toBeLessThanOrEqual(105);
    }
    expectFinite(batch);
  });

  it("extends a segment past both ends so the round caps are not clipped", () => {
    const item = strokeItem({ strokeWidth: 4 });
    const geometry = geometryFor(STROKE_TOOL.pen, [
      { u: 0.1, v: 0.1, pressure: 0 },
      { u: 0.9, v: 0.1, pressure: 0 },
    ]);

    const batch = buildStrokeBatch([entry(item, geometry)], { pixelsPerWorldUnit: 1 });

    const xs = [0, 1, 2, 3].map((index) => vertex(batch, index)[0]);
    // 半径 2 + 羽化 1：水平线段的两端各外扩 3，圆头才不会被四边形切平。
    expect(Math.min(...xs)).toBeCloseTo(7);
    expect(Math.max(...xs)).toBeCloseTo(93);
  });

  it("decimates sub-pixel points so a long stroke stays connected at low zoom", () => {
    const samples = Array.from({ length: 60 }, (_, index) => ({
      u: index / 59,
      v: 0,
      pressure: 0,
    }));
    const item = strokeItem({ width: 100, height: 100 });
    const geometry = geometryFor(STROKE_TOOL.pen, samples);

    // 100 世界单位宽、每单位 1 像素：60 个点两两相距约 1.7 px，抽稀后明显变少。
    const dense = buildStrokeBatch([entry(item, geometry)], { pixelsPerWorldUnit: 1 });
    // 每单位 0.005 像素（≈ 缩小到 0.5%）：合并阈值超过整笔长度，只剩首尾两点。
    const sparse = buildStrokeBatch([entry(item, geometry)], { pixelsPerWorldUnit: 0.005 });

    expect(dense.primitiveCount).toBeGreaterThan(30);
    expect(sparse.primitiveCount).toBe(1);
    expect(sparse.strokeCount).toBe(1);
    expectFinite(sparse);
  });

  it("insets line endpoints by half the stroke width", () => {
    const anchors = shapeAnchors({ width: 100, height: 40 }, 0, 2);

    expect(anchors[0]).toEqual([2, 2]);
    expect(anchors[1]).toEqual([98, 38]);
  });

  it("derives the opposite corner from the direction bits", () => {
    const item = { width: 100, height: 40 };

    expect(shapeAnchors(item, 1, 2)[0]).toEqual([98, 2]);
    expect(shapeAnchors(item, 2, 2)[0]).toEqual([98, 38]);
    expect(shapeAnchors(item, 3, 2)[0]).toEqual([2, 38]);
  });

  it("clamps the inset on a degenerate (zero-height) line box", () => {
    const anchors = shapeAnchors({ width: 100, height: 0 }, 0, 2);

    expect(anchors[0][1]).toBe(0);
    expect(anchors[1][1]).toBe(0);
  });

  it("renders a line as one segment primitive", () => {
    const batch = buildStrokeBatch(
      [entry(strokeItem({ tool: STROKE_TOOL.line, height: 40 }), geometryFor(STROKE_TOOL.line))],
      { pixelsPerWorldUnit: 1 },
    );

    expect(batch.primitiveCount).toBe(1);
    expect(vertex(batch, 0)[16]).toBe(STROKE_PRIMITIVE_KIND.segment);
    // 线宽 2 → 半径 1。
    expect(vertex(batch, 0)[10]).toBe(1);
  });

  it("adds a filled triangle to the arrow shaft", () => {
    const item = strokeItem({ tool: STROKE_TOOL.arrow, height: 40, strokeWidth: 4 });
    const batch = buildStrokeBatch([entry(item, geometryFor(STROKE_TOOL.arrow))], {
      pixelsPerWorldUnit: 1,
    });

    expect(batch.primitiveCount).toBe(2);
    expect(vertex(batch, 0)[16]).toBe(STROKE_PRIMITIVE_KIND.segment);
    expect(vertex(batch, 4)[16]).toBe(STROKE_PRIMITIVE_KIND.triangle);
    // 杆止于三角形底边（箭尖之前），杆末端的圆头不会从箭尖后面冒出来。
    const shaftEnd = vertex(batch, 0)[6];
    expect(shaftEnd).toBeLessThan(98);
    expect(shaftEnd).toBeGreaterThan(80);
    expectFinite(batch);
  });

  it("insets the rect path so the ink fills the item box", () => {
    const item = strokeItem({ tool: STROKE_TOOL.rect, strokeWidth: 6 });
    const batch = buildStrokeBatch([entry(item, geometryFor(STROKE_TOOL.rect))], {
      pixelsPerWorldUnit: 1,
    });

    const values = vertex(batch, 0);
    expect(values[16]).toBe(STROKE_PRIMITIVE_KIND.box);
    // a = 路径左上角（内缩半个线宽），b = 路径右下角。
    expect(values[4]).toBe(3);
    expect(values[5]).toBe(3);
    expect(values[6]).toBe(97);
    expect(values[7]).toBe(97);
    expect(values[10]).toBe(3);
  });

  it("uses the box center and radii for an ellipse", () => {
    const item = strokeItem({ tool: STROKE_TOOL.ellipse, strokeWidth: 6 });
    const batch = buildStrokeBatch([entry(item, geometryFor(STROKE_TOOL.ellipse))], {
      pixelsPerWorldUnit: 1,
    });

    const values = vertex(batch, 0);
    expect(values[16]).toBe(STROKE_PRIMITIVE_KIND.ellipse);
    expect(values[4]).toBe(50);
    expect(values[5]).toBe(50);
    expect(values[6]).toBe(47);
    expect(values[7]).toBe(47);
  });

  it("survives a stroke whose points are all identical", () => {
    const geometry = geometryFor(STROKE_TOOL.pen, [
      { u: 0.5, v: 0.5, pressure: 0 },
      { u: 0.5, v: 0.5, pressure: 0 },
    ]);

    const batch = buildStrokeBatch([entry(strokeItem(), geometry)], { pixelsPerWorldUnit: 1 });

    expect(batch.primitiveCount).toBe(0);
    expect(batch.indices).toHaveLength(0);
  });

  it("converts the hex colour into straight-alpha floats", () => {
    const batch = buildStrokeBatch(
      [entry(strokeItem({ color: "#FF8000" }), geometryFor(STROKE_TOOL.rect))],
      { pixelsPerWorldUnit: 1 },
    );

    const values = vertex(batch, 0);
    expect(values[12]).toBeCloseTo(1);
    expect(values[13]).toBeCloseTo(128 / 255, 5);
    expect(values[14]).toBe(0);
    expect(values[15]).toBe(1);
  });

  it("keeps pressure-driven radii inside the tapered range", () => {
    expect(strokePointRadius(10, 0, false)).toBe(5);
    // 压感为 0 视为「设备没给数据」，回退到固定线宽而不是最细。
    expect(strokePointRadius(10, 0, true)).toBe(5);
    expect(strokePointRadius(10, 1, true)).toBe(5);
    expect(strokePointRadius(10, 0.5, true)).toBeCloseTo(5 * (0.15 + 0.85 * 0.5));
  });
});

describe("pin-board stroke selection outline", () => {
  it("emits a closed 5-vertex strip per stroke and keeps the slot order", () => {
    const items = [
      { itemId: 7, points: BOX_POINTS },
      { itemId: 9, points: [[0, 0], [10, 0], [10, 10], [0, 10]] as Quad },
    ];

    const { vertices, index } = buildStrokeOutlines(items);

    expect(index.get(7)).toBe(0);
    expect(index.get(9)).toBe(1);
    expect(vertices).toHaveLength(2 * STROKE_OUTLINE_VERTICES * 2);
    const first = Array.from(vertices.subarray(0, 10)) as unknown as Point[];
    expect(first[0]).toBe(0);
    expect(first[9]).toBe(0);
    // 第 5 个顶点回到起点，line-strip 因此闭合。
    expect([vertices[8], vertices[9]]).toEqual([0, 0]);
  });
});
