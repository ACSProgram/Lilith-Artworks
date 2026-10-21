import { describe, expect, it } from "vitest";
import { decodeStrokeHeader, MAX_STROKE_POINTS, STROKE_TOOL } from "./strokeGeometry";
import { decodeStrokeSamples, shapeAnchors } from "./strokeRender";
import {
  DEFAULT_STROKE_WIDTH,
  MAX_STROKE_WIDTH,
  MIN_STROKE_WIDTH,
  STROKE_WIDTH_SLIDER_STEPS,
  beginStrokeDraft,
  clampStrokeWidth,
  constrainShapeEnd,
  extendStrokeDraft,
  resolveStrokeDraft,
  strokeDraftPoints,
  strokeWidthFromSlider,
  strokeWidthToSlider,
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

  it("maps the width slider exponentially so the thin end keeps its travel", () => {
    // 两端必须正好落在范围内。
    expect(strokeWidthFromSlider(0)).toBe(MIN_STROKE_WIDTH);
    expect(strokeWidthFromSlider(STROKE_WIDTH_SLIDER_STEPS)).toBe(MAX_STROKE_WIDTH);
    // 单调不减，且中段远小于线性映射的 (MIN + MAX) / 2。
    let previous = 0;
    for (let step = 0; step <= STROKE_WIDTH_SLIDER_STEPS; step += 25) {
      const width = strokeWidthFromSlider(step);
      expect(width).toBeGreaterThanOrEqual(previous);
      previous = width;
    }
    const middle = strokeWidthFromSlider(STROKE_WIDTH_SLIDER_STEPS / 2);
    expect(middle).toBeLessThan((MIN_STROKE_WIDTH + MAX_STROKE_WIDTH) / 2);
    // 1–20 这段最常调，应当占到足够的行程（本映射下约 38%）。
    expect(strokeWidthToSlider(20)).toBeGreaterThan(STROKE_WIDTH_SLIDER_STEPS * 0.3);
    // 粗线端不能挤在最后一小段里：200–500 至少占两成行程，右半段才不会一拖就窜。
    expect(STROKE_WIDTH_SLIDER_STEPS - strokeWidthToSlider(200)).toBeGreaterThan(
      STROKE_WIDTH_SLIDER_STEPS * 0.18,
    );
    // 往返一致：滑条位置 → 线宽 → 滑条位置不会跑掉。
    for (const width of [1, 2, 6, 20, 64, 120, 500]) {
      expect(strokeWidthFromSlider(strokeWidthToSlider(width))).toBe(width);
    }
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
    // 墨迹外框：点中心极值 ± 半个线宽。光滑后的曲线会略微过冲，因此留一点余量。
    expect(resolved.width).toBeCloseTo(220 + WIDTH, 0);
    expect(resolved.height).toBeCloseTo(200 + WIDTH, 0);
    expect(resolved.centerX).toBeCloseTo(210, 0);
    expect(resolved.centerY).toBeCloseTo(300, 0);
    const quad = commitQuad(resolved);
    const world = penSamples(resolved).map((sample) => quad.toWorld(sample));
    // 首尾点原样落在用户按下的位置与抬笔的位置。
    expect(world[0][0]).toBeCloseTo(100, 2);
    expect(world[0][1]).toBeCloseTo(200, 2);
    expect(world[world.length - 1][0]).toBeCloseTo(320, 2);
    expect(world[world.length - 1][1]).toBeCloseTo(400, 2);
    // 归一化后仍在包围盒内：不会因为光滑而溢出选中框。
    for (const sample of penSamples(resolved)) {
      expect(sample[0]).toBeGreaterThanOrEqual(-1e-4);
      expect(sample[0]).toBeLessThanOrEqual(1 + 1e-4);
      expect(sample[1]).toBeGreaterThanOrEqual(-1e-4);
      expect(sample[1]).toBeLessThanOrEqual(1 + 1e-4);
    }
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
    expect(draft.samples).toHaveLength(2);
  });

  it("thins strokes that exceed the point limit instead of rejecting them", () => {
    const points: Point[] = Array.from({ length: MAX_STROKE_POINTS + 900 }, (_, index) => (
      [index, index % 17] as Point
    ));
    const commit = resolveStrokeDraft(draftOf("pen", points), WIDTH)!;
    expect(decodeStrokeHeader(commit.geometry).pointCount).toBeLessThanOrEqual(MAX_STROKE_POINTS);
    const world = penSamples(commit).map((sample) => commitQuad(commit).toWorld(sample));
    expect(world[0][0]).toBeCloseTo(0, 0);
    expect(world[world.length - 1][0]).toBeCloseTo(points[points.length - 1][0], 0);
  });
});

describe("pin-board pen pressure", () => {
  function penPressures(commit: StrokeCommit): number[] {
    return decodeStrokeSamples({
      ...decodeStrokeHeader(commit.geometry),
      itemId: 1,
      color: "#FFFFFF",
      strokeWidth: commit.strokeWidth,
      width: commit.width,
      height: commit.height,
      geometry: commit.geometry,
    }).map((sample) => sample.pressure);
  }

  it("writes the pressure flag only for a pen with real pressure data", () => {
    const pen = beginStrokeDraft("pen", [0, 0], { pressure: 0.4, pointerType: "pen" });
    extendStrokeDraft(pen, [100, 0], false, { pressure: 0.9, pointerType: "pen" });
    const penCommit = resolveStrokeDraft(pen, WIDTH)!;
    expect(decodeStrokeHeader(penCommit.geometry).hasPressure).toBe(true);
    const pressures = penPressures(penCommit);
    // 两端 taper 收细，中段保持原始压感：不会整笔被压成一个宽度。
    expect(pressures[0]).toBeLessThan(pressures[Math.floor(pressures.length / 2)]);
    expect(Math.max(...pressures)).toBeGreaterThan(0.6);

    // 鼠标的 `pressure` 恒为 0.5 且没有意义：整笔按固定线宽，不写压感标志。
    const mouse = beginStrokeDraft("pen", [0, 0], { pressure: 0.5, pointerType: "mouse" });
    extendStrokeDraft(mouse, [100, 0], false, { pressure: 0.5, pointerType: "mouse" });
    expect(decodeStrokeHeader(resolveStrokeDraft(mouse, WIDTH)!.geometry).hasPressure).toBe(false);
  });

  it("keeps the pressure of the final sample on a shift-constrained line", () => {
    const draft = beginStrokeDraft("pen", [0, 0], { pressure: 0.2, pointerType: "pen" });
    extendStrokeDraft(draft, [50, 50], false, { pressure: 0.2, pointerType: "pen" });
    extendStrokeDraft(draft, [100, 10], true, { pressure: 0.9, pointerType: "pen" });
    expect(strokeDraftPoints(draft)).toEqual([[0, 0], [100, 10]]);
    const commit = resolveStrokeDraft(draft, WIDTH)!;
    const header = decodeStrokeHeader(commit.geometry);
    expect(header.pointCount).toBe(2);
    expect(header.hasPressure).toBe(true);
    expect(penPressures(commit)[1]).toBeGreaterThan(penPressures(commit)[0]);
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
