import { describe, expect, it } from "vitest";
import { decodeStrokeHeader, MAX_STROKE_POINTS, STROKE_TOOL } from "./strokeGeometry";
import { decodeStrokeSamples, shapeAnchors } from "./strokeRender";
import {
  DEFAULT_STROKE_WIDTH,
  MAX_STROKE_WIDTH,
  MIN_STROKE_WIDTH,
  beginStrokeDraft,
  clampStrokeWidth,
  constrainShapeEnd,
  extendStrokeDraft,
  resolveStrokeDraft,
  strokeDraftPoints,
  thinStrokePoints,
  toolForShortcut,
  type StrokeCommit,
  type StrokeToolName,
} from "./strokeDrawing";
import type { Point } from "./geometry";

const WIDTH = 6;

/** 把提交结果还原成「条目四角 + 局部坐标」，用于验证读写两侧的包围盒口径一致。 */
function commitQuad(commit: StrokeCommit) {
  const left = commit.centerX - commit.width / 2;
  const top = commit.centerY + commit.height / 2;
  return {
    left,
    top,
    width: commit.width,
    height: commit.height,
    /** 局部（包围盒内归一化）坐标 → 世界坐标，与渲染层同一口径。 */
    toWorld: ([u, v]: Point): Point => [left + u * commit.width, top - v * commit.height],
  };
}

function penSamples(commit: StrokeCommit): Point[] {
  const header = decodeStrokeHeader(commit.geometry);
  const samples = decodeStrokeSamples({
    ...header,
    itemId: 1,
    color: "#FFFFFF",
    strokeWidth: commit.strokeWidth,
    width: commit.width,
    height: commit.height,
    geometry: commit.geometry,
  });
  return samples.map((sample) => [sample.u, sample.v] as Point);
}

function draftOf(tool: StrokeToolName, points: Point[], shift = false) {
  const draft = beginStrokeDraft(tool, points[0]);
  for (const point of points.slice(1)) extendStrokeDraft(draft, point, shift);
  return draft;
}

describe("pin-board drawing tools", () => {
  it("maps single-key shortcuts and ignores unknown keys", () => {
    expect(toolForShortcut("v")).toBe("select");
    expect(toolForShortcut("P")).toBe("pen");
    expect(toolForShortcut("l")).toBe("line");
    expect(toolForShortcut("A")).toBe("arrow");
    expect(toolForShortcut("r")).toBe("rect");
    expect(toolForShortcut("o")).toBe("ellipse");
    expect(toolForShortcut("x")).toBeNull();
  });

  it("clamps stroke widths to the input range", () => {
    expect(clampStrokeWidth(0)).toBe(MIN_STROKE_WIDTH);
    expect(clampStrokeWidth(10_000)).toBe(MAX_STROKE_WIDTH);
    expect(clampStrokeWidth(Number.NaN)).toBe(DEFAULT_STROKE_WIDTH);
    expect(clampStrokeWidth(12)).toBe(12);
  });
});

describe("pin-board pen drafts", () => {
  it("inflates the bounding box by the stroke radius and keeps points inside", () => {
    const commit = resolveStrokeDraft(
      draftOf("pen", [[100, 200], [300, 260], [320, 400]]),
      WIDTH,
    );
    expect(commit).not.toBeNull();
    const resolved = commit!;
    // 墨迹外框：点中心极值 ± 半个线宽。
    expect(resolved.width).toBeCloseTo(220 + WIDTH, 6);
    expect(resolved.height).toBeCloseTo(200 + WIDTH, 6);
    expect(resolved.centerX).toBeCloseTo(210, 6);
    expect(resolved.centerY).toBeCloseTo(300, 6);
    const quad = commitQuad(resolved);
    const world = penSamples(resolved).map((sample) => quad.toWorld(sample));
    expect(world[0][0]).toBeCloseTo(100, 2);
    expect(world[0][1]).toBeCloseTo(200, 2);
    expect(world[2][0]).toBeCloseTo(320, 2);
    expect(world[2][1]).toBeCloseTo(400, 2);
  });

  it("keeps a horizontal stroke from collapsing to zero height", () => {
    const commit = resolveStrokeDraft(draftOf("pen", [[0, 0], [400, 0]]), WIDTH)!;
    expect(commit.height).toBeCloseTo(WIDTH, 6);
    const quad = commitQuad(commit);
    const world = penSamples(commit).map((sample) => quad.toWorld(sample));
    // 归一化后仍在包围盒内：v 恒为 0.5，不会出现除零（u16 量化误差在 0.01 以内）。
    expect(world.every(([, y]) => Math.abs(y) < 0.01)).toBe(true);
  });

  it("treats a click as a cancelled stroke", () => {
    const draft = draftOf("pen", [[50, 50]]);
    expect(resolveStrokeDraft(draft, WIDTH)).toBeNull();
    expect(resolveStrokeDraft(draft, WIDTH, 3)).toBeNull();
  });

  it("collapses to a straight line while shift is held", () => {
    const draft = beginStrokeDraft("pen", [0, 0]);
    extendStrokeDraft(draft, [10, 10], false);
    extendStrokeDraft(draft, [80, 20], true);
    expect(strokeDraftPoints(draft)).toEqual([[0, 0], [80, 20]]);
    const commit = resolveStrokeDraft(draft, WIDTH)!;
    const world = penSamples(commit).map((sample) => commitQuad(commit).toWorld(sample));
    expect(world).toHaveLength(2);
    expect(world[1][0]).toBeCloseTo(80, 2);
  });

  it("drops repeated samples that arrive at the same position", () => {
    const draft = beginStrokeDraft("pen", [0, 0]);
    extendStrokeDraft(draft, [0, 0], false);
    extendStrokeDraft(draft, [10, 0], false);
    extendStrokeDraft(draft, [10, 0], false);
    expect(draft.points).toHaveLength(2);
  });

  it("thins strokes that exceed the point limit instead of rejecting them", () => {
    const points: Point[] = Array.from({ length: MAX_STROKE_POINTS + 900 }, (_, index) => (
      [index, index % 17] as Point
    ));
    const thinned = thinStrokePoints(points);
    expect(thinned).toHaveLength(MAX_STROKE_POINTS);
    expect(thinned[0]).toEqual(points[0]);
    expect(thinned[thinned.length - 1]).toEqual(points[points.length - 1]);
    const commit = resolveStrokeDraft(draftOf("pen", points), WIDTH)!;
    expect(decodeStrokeHeader(commit.geometry).pointCount).toBe(MAX_STROKE_POINTS);
  });
});

describe("pin-board shape drafts", () => {
  const cases: Array<{ start: Point; end: Point; direction: number; label: string }> = [
    { start: [0, 100], end: [100, 0], direction: 0, label: "左上 → 右下" },
    { start: [100, 100], end: [0, 0], direction: 1, label: "右上 → 左下" },
    { start: [100, 0], end: [0, 100], direction: 2, label: "右下 → 左上" },
    { start: [0, 0], end: [100, 100], direction: 3, label: "左下 → 右上" },
  ];

  for (const item of cases) {
    it(`encodes the direction bit for ${item.label}`, () => {
      const commit = resolveStrokeDraft(draftOf("line", [item.start, item.end]), WIDTH)!;
      const header = decodeStrokeHeader(commit.geometry);
      expect(header.tool).toBe(STROKE_TOOL.line);
      expect(header.direction).toBe(item.direction);
      expect(header.pointCount).toBe(0);
      // 读侧从包围盒 + 方向位还原出的两端点（局部坐标）必须回到用户拖拽的位置。
      const anchors = shapeAnchors(commit, header.direction, WIDTH / 2);
      const quad = commitQuad(commit);
      const localToWorld = ([x, y]: Point): Point => [quad.left + x, quad.top - y];
      const start = localToWorld(anchors[0]);
      const end = localToWorld(anchors[1]);
      expect(start[0]).toBeCloseTo(item.start[0], 6);
      expect(start[1]).toBeCloseTo(item.start[1], 6);
      expect(end[0]).toBeCloseTo(item.end[0], 6);
      expect(end[1]).toBeCloseTo(item.end[1], 6);
    });
  }

  it("inflates the box by the full stroke width for a horizontal line", () => {
    const commit = resolveStrokeDraft(draftOf("arrow", [[0, 0], [300, 0]]), WIDTH)!;
    expect(commit.width).toBeCloseTo(300 + WIDTH, 6);
    expect(commit.height).toBeCloseTo(WIDTH, 6);
    expect(commit.centerY).toBeCloseTo(0, 6);
  });

  it("snaps a shifted line to 45 degree steps", () => {
    const snapped = constrainShapeEnd("line", [0, 0], [100, 10], true);
    expect(snapped[0]).toBeCloseTo(100, 6);
    expect(snapped[1]).toBeCloseTo(0, 6);
    const diagonal = constrainShapeEnd("arrow", [0, 0], [100, 90], true);
    const angle = Math.atan2(diagonal[1], diagonal[0]);
    expect(angle).toBeCloseTo(Math.PI / 4, 6);
  });

  it("makes rectangles square and ellipses round while shift is held", () => {
    const rect = resolveStrokeDraft(draftOf("rect", [[0, 0], [200, 60]], true), WIDTH)!;
    expect(rect.width - WIDTH).toBeCloseTo(200, 6);
    expect(rect.height - WIDTH).toBeCloseTo(200, 6);
    const ellipse = resolveStrokeDraft(draftOf("ellipse", [[0, 0], [-40, -90]], true), WIDTH)!;
    expect(ellipse.width - WIDTH).toBeCloseTo(90, 6);
    expect(ellipse.height - WIDTH).toBeCloseTo(90, 6);
    // 方向沿用拖拽方向：起点在右上。
    expect(decodeStrokeHeader(ellipse.geometry).direction).toBe(1);
  });

  it("cancels shapes that were not dragged far enough", () => {
    const draft = draftOf("rect", [[0, 0], [0, 0]]);
    expect(resolveStrokeDraft(draft, WIDTH, 3)).toBeNull();
  });
});
