import { describe, expect, it } from "vitest";
import {
  decodeStrokeHeader,
  encodeStrokeGeometry,
  STROKE_TOOL,
  type PinBoardStrokeGeometry,
} from "./strokeGeometry";
import {
  STROKE_OUTLINE_VERTICES,
  STROKE_PRIMITIVE_KIND,
  STROKE_VERTEX_FLOATS,
  buildStrokeBatch,
  buildStrokeOutlines,
  decodeStrokeSamples,
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
  it("builds one mitered quad per pen segment", () => {
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

  it("shares join vertices between neighbouring segments", () => {
    // 三段折线：相邻段的接缝必须用同一对顶点，重叠一次就会让细线在采样间隔上发花。
    const item = strokeItem({ width: 100, height: 100, strokeWidth: 4 });
    const geometry = geometryFor(STROKE_TOOL.pen, [
      { u: 0.1, v: 0.1, pressure: 0 },
      { u: 0.5, v: 0.5, pressure: 0 },
      { u: 0.9, v: 0.1, pressure: 0 },
    ]);

    const batch = buildStrokeBatch([entry(item, geometry)], { pixelsPerWorldUnit: 1 });

    expect(batch.primitiveCount).toBe(2);
    // 每段 4 个顶点，顺序是 [起点左, 终点左, 终点右, 起点右]。
    const endLeft = vertex(batch, 1);
    const endRight = vertex(batch, 2);
    const nextStartLeft = vertex(batch, 4);
    const nextStartRight = vertex(batch, 7);
    expect(endLeft.slice(0, 4)).toEqual(nextStartLeft.slice(0, 4));
    expect(endRight.slice(0, 4)).toEqual(nextStartRight.slice(0, 4));
    // 斜接顶点比圆头更靠外：转角处两段的外侧顶点到采样点的距离大于半径。
    const radius = endLeft[10];
    const jointDistance = Math.hypot(endLeft[2] - 50, endLeft[3] - 50);
    expect(jointDistance).toBeGreaterThan(radius);
  });

  it("extends the stroke ends past the caps so the round ends are not clipped", () => {
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
    // 四边形必须比路径再外扩「半个线宽 + 羽化余量」：路径已在 3，墨迹外沿在 0，
    // 四边形要落到 -1，否则外侧半个线宽会被裁掉（落地后矩形突然变细就是这条漏了）。
    const xs = [0, 1, 2, 3].map((index) => vertex(batch, index)[2]);
    const ys = [0, 1, 2, 3].map((index) => vertex(batch, index)[3]);
    expect(Math.min(...xs)).toBeCloseTo(-1);
    expect(Math.max(...xs)).toBeCloseTo(101);
    expect(Math.min(...ys)).toBeCloseTo(-1);
    expect(Math.max(...ys)).toBeCloseTo(101);
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
    // 墨迹外沿在 50 ± 50：四边形必须完全盖住它，否则椭圆会被自己的方框切掉两端。
    const xs = [0, 1, 2, 3].map((index) => vertex(batch, index)[2]);
    const ys = [0, 1, 2, 3].map((index) => vertex(batch, index)[3]);
    expect(Math.min(...xs)).toBeLessThanOrEqual(0);
    expect(Math.max(...xs)).toBeGreaterThanOrEqual(100);
    expect(Math.min(...ys)).toBeLessThanOrEqual(0);
    expect(Math.max(...ys)).toBeGreaterThanOrEqual(100);
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
    // 整笔没有压感时才回退到固定线宽；笔迹内部的 0 就是「几乎没使劲」，按最细一档画。
    expect(strokePointRadius(10, 0, true)).toBeCloseTo(5 * 0.15);
    expect(strokePointRadius(10, 1, true)).toBe(5);
    expect(strokePointRadius(10, 0.5, true)).toBeCloseTo(5 * (0.15 + 0.85 * 0.5));
  });

  it("skips primitives that fall outside the cull bounds", () => {
    const items = [entry(
      strokeItem({ tool: STROKE_TOOL.line }),
      geometryFor(STROKE_TOOL.line),
    )];
    const options = { pixelsPerWorldUnit: 1 };

    const whole = buildStrokeBatch(items, options);
    const culled = buildStrokeBatch(items, {
      ...options,
      cullBounds: { minX: 1000, minY: 1000, maxX: 2000, maxY: 2000 },
    });

    expect(whole.indices.length).toBeGreaterThan(0);
    // 整条直线都在裁剪框外：一个图元都不写，调用方跳过这一趟绘制。
    expect(culled.indices).toHaveLength(0);
    expect(culled.strokeCount).toBe(0);
  });

  it("culls a long freehand stroke segment by segment across the viewport", () => {
    // 一条横贯 1000 世界单位长的自由笔（1000 段），视口只看得到 x ∈ [400, 600]。
    const samples = Array.from({ length: 1001 }, (_, index) => ({
      u: index / 1000,
      v: 0.5,
      pressure: 0,
    }));
    const item = strokeItem({
      width: 1000,
      height: 10,
      points: [[0, 0], [1000, 0], [1000, 10], [0, 10]],
    });
    const items = [entry(item, geometryFor(STROKE_TOOL.pen, samples))];
    // pixelsPerWorldUnit = 10 让抽稀阈值（1/10 世界单位）远小于点距（1 世界单位），
    // 1001 个采样点全部保留——这里测的是裁剪，不是 LOD。
    const options = { pixelsPerWorldUnit: 10 };
    const cullBounds = { minX: 400, minY: -100, maxX: 600, maxY: 100 };

    const whole = buildStrokeBatch(items, options);
    const culled = buildStrokeBatch(items, { ...options, cullBounds });

    expect(whole.primitiveCount).toBe(1000);
    // 视口只覆盖 20% 的长度：大部分段被裁掉，但不是整笔消失。
    expect(culled.primitiveCount).toBeGreaterThan(0);
    expect(culled.primitiveCount).toBeLessThan(whole.primitiveCount / 2);
    expect(culled.strokeCount).toBe(1);
    expect(culled.indices).toHaveLength(culled.primitiveCount * 6);

    // 每个写出的四边形都必须与视口相交，且至少各有一段跨越左右边界——
    // 「逐段裁剪」意味着框内被完整覆盖，而不是只画了框内整段的某个子集。
    let minX = Number.POSITIVE_INFINITY;
    let maxX = Number.NEGATIVE_INFINITY;
    for (let primitive = 0; primitive < culled.primitiveCount; primitive += 1) {
      const xs: number[] = [];
      for (let corner = 0; corner < 4; corner += 1) {
        xs.push(vertex(culled, primitive * 4 + corner)[0]);
      }
      const left = Math.min(...xs);
      const right = Math.max(...xs);
      expect(right).toBeGreaterThanOrEqual(cullBounds.minX);
      expect(left).toBeLessThanOrEqual(cullBounds.maxX);
      minX = Math.min(minX, left);
      maxX = Math.max(maxX, right);
    }
    expect(minX).toBeLessThanOrEqual(cullBounds.minX + 5);
    expect(maxX).toBeGreaterThanOrEqual(cullBounds.maxX - 5);
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
