import { afterEach, describe, expect, it, vi } from "vitest";
import {
  installStrokePerfProbe,
  runStrokePerfProbe,
  syntheticStroke,
} from "./strokePerfProbe";

describe("pin-board stroke perf probe", () => {
  afterEach(() => vi.restoreAllMocks());

  it("generates deterministic synthetic samples", () => {
    const first = syntheticStroke(3, 8);
    const second = syntheticStroke(3, 8);
    expect(first).toEqual(second);
    expect(first).toHaveLength(8);
    expect(first[0].point[0]).toBeCloseTo(120, 6);
    expect(first.every((sample) => sample.pressure > 0 && sample.pressure <= 1)).toBe(true);
    // 相邻笔迹不重合。
    expect(syntheticStroke(4, 8)[0].point[0]).not.toBeCloseTo(first[0].point[0], 6);
  });

  it("encodes and builds a synthetic board in memory", () => {
    const result = runStrokePerfProbe({ strokes: 20, pointsPerStroke: 60 });
    expect(result.strokes).toBe(20);
    expect(result.sampledPoints).toBe(20 * 60);
    // 采样管道会抽稀并重采样：点数是正数，且落在单笔上限以内。
    expect(result.encodedPoints).toBeGreaterThan(0);
    expect(result.encodedPoints / result.strokes).toBeLessThanOrEqual(4096);
    expect(result.geometryBytes).toBeGreaterThan(result.encodedPoints);
    expect(result.primitives).toBeGreaterThan(0);
    expect(result.vertices).toBe(result.primitives * 4);
    for (const value of [result.samplingMs, result.encodeMs, result.buildMs]) {
      expect(Number.isFinite(value)).toBe(true);
      expect(value).toBeGreaterThanOrEqual(0);
    }
  });

  it("installs a callable console entry", () => {
    const table = vi.spyOn(console, "table").mockImplementation(() => undefined);
    installStrokePerfProbe();
    const probe = (globalThis as unknown as {
      __lilithPinBoardStrokeProbe?: (options?: unknown) => ReturnType<typeof runStrokePerfProbe>;
    }).__lilithPinBoardStrokeProbe;
    expect(typeof probe).toBe("function");
    const result = probe!({ strokes: 2, pointsPerStroke: 6 });
    expect(result.strokes).toBe(2);
    expect(result.primitives).toBeGreaterThan(0);
  });
});
