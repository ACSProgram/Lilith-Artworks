import { describe, expect, it } from "vitest";
import type { Point } from "./geometry";
import { MAX_STROKE_POINTS } from "./strokeGeometry";
import {
  CORNER_ANGLE_RADIANS,
  SAMPLE_STEP_CSS_PIXELS,
  applyTaper,
  catmullRomControlPoints,
  flattenRun,
  limitSamples,
  normalisePenSamples,
  polylineLength,
  resampleByArcLength,
  simplifyPolyline,
  smoothPressures,
  splitAtCorners,
  type PressureSample,
} from "./strokeSampling";

function sample(point: Point, pressure = 0): PressureSample {
  return { point, pressure };
}

/** 沿圆弧取 `count` 个点：相邻间距固定，模拟 60 Hz 的稳定采样。 */
function arc(count: number, radius = 200, from = 0, to = Math.PI): PressureSample[] {
  return Array.from({ length: count }, (_, index) => {
    const angle = from + ((to - from) * index) / (count - 1);
    return sample([Math.cos(angle) * radius, Math.sin(angle) * radius], 0.5);
  });
}

function minDistance(point: Point, samples: readonly PressureSample[]): number {
  return samples.reduce(
    (best, entry) => Math.min(best, Math.hypot(entry.point[0] - point[0], entry.point[1] - point[1])),
    Number.POSITIVE_INFINITY,
  );
}

/** 点到折线（而不是到保留点）的距离：抽稀后相邻保留点之间可以隔着很远。 */
function distanceToPolyline(point: Point, samples: readonly PressureSample[]): number {
  let best = Number.POSITIVE_INFINITY;
  for (let index = 1; index < samples.length; index += 1) {
    const start = samples[index - 1].point;
    const end = samples[index].point;
    const deltaX = end[0] - start[0];
    const deltaY = end[1] - start[1];
    const lengthSquared = deltaX * deltaX + deltaY * deltaY;
    const ratio = lengthSquared > 0
      ? Math.min(1, Math.max(0, ((point[0] - start[0]) * deltaX + (point[1] - start[1]) * deltaY) / lengthSquared))
      : 0;
    best = Math.min(best, Math.hypot(
      point[0] - (start[0] + deltaX * ratio),
      point[1] - (start[1] + deltaY * ratio),
    ));
  }
  return best;
}

function bounds(samples: readonly PressureSample[]) {
  return samples.reduce((accumulator, entry) => ({
    minX: Math.min(accumulator.minX, entry.point[0]),
    minY: Math.min(accumulator.minY, entry.point[1]),
    maxX: Math.max(accumulator.maxX, entry.point[0]),
    maxY: Math.max(accumulator.maxY, entry.point[1]),
  }), {
    minX: Number.POSITIVE_INFINITY,
    minY: Number.POSITIVE_INFINITY,
    maxX: Number.NEGATIVE_INFINITY,
    maxY: Number.NEGATIVE_INFINITY,
  });
}

describe("pin-board stroke sampling", () => {
  it("cuts more than half of a dense 60 Hz arc while keeping its shape", () => {
    const raw = arc(120);
    const simplified = simplifyPolyline(raw, 1);
    expect(simplified.length).toBeLessThan(raw.length / 2);
    // 抽稀不能改变形状：每个被丢掉的原点都贴着保留点的折线。
    expect(distanceToPolyline(raw[60].point, simplified)).toBeLessThan(1.5);
    expect(simplified[0]).toEqual(raw[0]);
    expect(simplified[simplified.length - 1]).toEqual(raw[raw.length - 1]);
  });

  it("keeps every original point within a sample step of the smoothed curve", () => {
    const raw = arc(60);
    const smoothed = normalisePenSamples(raw, {
      pixelsPerWorldUnit: 1,
      strokeWidth: 6,
      usePressure: false,
    });
    for (const entry of raw) {
      expect(minDistance(entry.point, smoothed)).toBeLessThan(SAMPLE_STEP_CSS_PIXELS * 1.5);
    }
    expect(smoothed[0].point[0]).toBeCloseTo(raw[0].point[0], 6);
    expect(smoothed[smoothed.length - 1].point[1]).toBeCloseTo(raw[raw.length - 1].point[1], 6);
  });

  it("keeps the interpolation overshoot negligible", () => {
    const raw: PressureSample[] = [
      sample([0, 0]),
      sample([100, 0]),
      sample([200, 40]),
      sample([300, 0]),
      sample([400, 0]),
    ];
    const smoothed = normalisePenSamples(raw, {
      pixelsPerWorldUnit: 1,
      strokeWidth: 4,
      usePressure: false,
    });
    const input = bounds(raw);
    const output = bounds(smoothed);
    // 向心参数化把过冲压到很小，但不是零：曲线在收尾处会略微低于起点 1% 以内。
    const margin = (input.maxX - input.minX) * 0.01;
    expect(output.minX).toBeGreaterThanOrEqual(input.minX - margin);
    expect(output.maxX).toBeLessThanOrEqual(input.maxX + margin);
    expect(output.minY).toBeGreaterThanOrEqual(input.minY - margin);
    expect(output.maxY).toBeLessThanOrEqual(input.maxY + margin);
  });

  it("splits runs at sharp corners instead of rounding them off", () => {
    const corner: PressureSample[] = [
      sample([0, 0]),
      sample([100, 0]),
      sample([100, 100]),
      sample([100, 200]),
    ];
    const runs = splitAtCorners(corner);
    expect(runs).toHaveLength(2);
    expect(runs[0][runs[0].length - 1].point).toEqual([100, 0]);
    expect(runs[1][0].point).toEqual([100, 0]);

    // 直角点必须原样出现在最终序列里：它既是两段的接缝，也不该被抹圆。
    const smoothed = normalisePenSamples(corner, {
      pixelsPerWorldUnit: 1,
      strokeWidth: 4,
      usePressure: false,
    });
    expect(minDistance([100, 0], smoothed)).toBeLessThan(1e-6);
    expect(turnAngleAt(smoothed, [100, 0])).toBeGreaterThan(CORNER_ANGLE_RADIANS * 0.9);
  });

  it("resamples at a uniform arc length and keeps the endpoints", () => {
    const dense = flattenRun([sample([0, 0]), sample([30, 0]), sample([60, 0])], 2);
    const resampled = resampleByArcLength(dense, 2);
    const gaps = resampled.slice(1).map((entry, index) => (
      Math.hypot(
        entry.point[0] - resampled[index].point[0],
        entry.point[1] - resampled[index].point[1],
      )
    ));
    expect(Math.max(...gaps)).toBeLessThan(2.01);
    expect(resampled[resampled.length - 1].point[0]).toBeCloseTo(60, 6);
  });

  it("keeps control points of a straight run on the line", () => {
    const [control1, control2] = catmullRomControlPoints([0, 0], [10, 0], [20, 0], [30, 0]);
    expect(control1[1]).toBeCloseTo(0, 9);
    expect(control2[1]).toBeCloseTo(0, 9);
    expect(control1[0]).toBeGreaterThan(10);
    expect(control2[0]).toBeLessThan(20);
  });

  it("leaves a two point stroke as a straight segment", () => {
    const smoothed = normalisePenSamples([sample([0, 0]), sample([300, 0])], {
      pixelsPerWorldUnit: 1,
      strokeWidth: 6,
      usePressure: false,
    });
    expect(smoothed).toHaveLength(2);
    expect(smoothed[1].point[0]).toBeCloseTo(300, 6);
  });
});

describe("pin-board stroke pressure", () => {
  it("smooths pressure spikes without moving the ends of the ramp", () => {
    const samples = [0, 0, 1, 0, 0].map((pressure, index) => sample([index * 10, 0], pressure));
    const smoothed = smoothPressures(samples, 1);
    expect(smoothed[2].pressure).toBeCloseTo(1 / 3, 6);
    expect(smoothed[0].pressure).toBeCloseTo(0, 6);
  });

  it("tapers both ends of a long pressure stroke", () => {
    const samples = Array.from({ length: 80 }, (_, index) => sample([index * 5, 0], 0.8));
    const tapered = applyTaper(samples, 8);
    const middle = tapered[40].pressure;
    expect(middle).toBeCloseTo(0.8, 6);
    expect(tapered[0].pressure).toBeLessThan(middle * 0.6);
    expect(tapered[tapered.length - 1].pressure).toBeLessThan(middle * 0.6);
    // 单调爬升：笔尖到中段不该有回跳。
    expect(tapered[1].pressure).toBeGreaterThanOrEqual(tapered[0].pressure);
  });

  it("skips the taper for short strokes so they are not thinned as a whole", () => {
    const samples = [sample([0, 0], 0.8), sample([4, 0], 0.8), sample([8, 0], 0.8), sample([12, 0], 0.8)];
    expect(applyTaper(samples, 8).map((entry) => entry.pressure)).toEqual([0.8, 0.8, 0.8, 0.8]);
  });

  it("keeps pressure through the pipeline and drops it when the device has none", () => {
    const raw = arc(40).map((entry) => sample(entry.point, 0.9));
    const withPressure = normalisePenSamples(raw, {
      pixelsPerWorldUnit: 1,
      strokeWidth: 6,
      usePressure: true,
    });
    const middle = withPressure[Math.floor(withPressure.length / 2)].pressure;
    expect(middle).toBeGreaterThan(0.5);
    expect(withPressure[0].pressure).toBeLessThan(middle);

    const withoutPressure = normalisePenSamples(raw, {
      pixelsPerWorldUnit: 1,
      strokeWidth: 6,
      usePressure: false,
    });
    expect(withoutPressure.every((entry) => entry.pressure === 0)).toBe(true);
  });
});

describe("pin-board stroke point budget", () => {
  it("never exceeds the point limit for a very long stroke", () => {
    const raw = Array.from({ length: 5000 }, (_, index) => (
      sample([index * 0.4, Math.sin(index / 12) * 40])
    ));
    const smoothed = normalisePenSamples(raw, {
      pixelsPerWorldUnit: 8,
      strokeWidth: 6,
      usePressure: true,
    });
    expect(smoothed.length).toBeLessThanOrEqual(MAX_STROKE_POINTS);
    expect(smoothed.length).toBeGreaterThan(64);
    expect(polylineLength(smoothed)).toBeGreaterThan(polylineLength(raw) * 0.95);
  });

  it("decimates uniformly when the limit is hit", () => {
    const samples = Array.from({ length: 100 }, (_, index) => sample([index, 0]));
    const limited = limitSamples(samples, 10);
    expect(limited).toHaveLength(10);
    expect(limited[0].point).toEqual([0, 0]);
    expect(limited[9].point).toEqual([99, 0]);
    expect(limited[5].point[0]).toBeCloseTo(55, 6);
  });
});

/** `point` 在序列里的转角，找不到时返回 0。 */
function turnAngleAt(samples: readonly PressureSample[], point: Point): number {
  const index = samples.findIndex((entry) => (
    Math.abs(entry.point[0] - point[0]) < 1e-6 && Math.abs(entry.point[1] - point[1]) < 1e-6
  ));
  if (index <= 0 || index >= samples.length - 1) return 0;
  const [previous, middle, next] = [samples[index - 1].point, samples[index].point, samples[index + 1].point];
  const inX = middle[0] - previous[0];
  const inY = middle[1] - previous[1];
  const outX = next[0] - middle[0];
  const outY = next[1] - middle[1];
  const cosine = (inX * outX + inY * outY)
    / (Math.hypot(inX, inY) * Math.hypot(outX, outY));
  return Math.acos(Math.min(1, Math.max(-1, cosine)));
}
