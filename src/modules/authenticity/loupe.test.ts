import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PreviewImage } from "./types";
import type { TileRequest } from "./previewTile";
import {
  LOUPE_FETCH_DEBOUNCE_MS, LOUPE_GAP, LOUPE_PREFETCH_DELAY_MS, LOUPE_REGION_EDGE, LOUPE_SIZE, LOUPE_SPAN_MAX,
  LOUPE_SPAN_MIN, LOUPE_ZOOM, loupeNeighbours, loupePlacement, loupeRegion, loupeSpan, loupeTarget, regionKey,
  useLoupeFrame,
} from "./loupe";

const base: PreviewImage = {
  dataUrl: "data:image/png;base64,YmFzZQ==",
  width: 4000,
  height: 3000,
  sourceBytes: 0,
};

const tile: PreviewImage = {
  dataUrl: "data:image/png;base64,dGlsZQ==",
  width: LOUPE_REGION_EDGE,
  height: LOUPE_REGION_EDGE,
  sourceBytes: 0,
};

/** 取样跨度；1:1 主视图下等于 LOUPE_SIZE / LOUPE_ZOOM，与旧的固定跨度一致。 */
const SPAN = LOUPE_SIZE / LOUPE_ZOOM;

function frameOptions(
  pointer: { x: number; y: number } | null,
  request: (tile: TileRequest) => Promise<PreviewImage>,
  sourceKey = "source-a",
) {
  return { pointer, sourceWidth: 4000, sourceHeight: 3000, span: SPAN, sourceKey, base, request };
}

const targetAt = (sourceX: number, sourceY: number) => loupeTarget(sourceX, sourceY, 4000, 3000, SPAN)!;

describe("loupeSpan", () => {
  it("keeps the loupe a fixed multiple of the main view", () => {
    // 1:1 主视图 → 与旧的固定 72 源像素一致。
    expect(loupeSpan(1)).toBeCloseTo(LOUPE_SIZE / LOUPE_ZOOM);
    // 跨度与显示比例成反比：主视图缩小一半，跨度翻倍。
    expect(loupeSpan(0.5)).toBeCloseTo(LOUPE_SIZE / (LOUPE_ZOOM * 0.5));
    expect(loupeSpan(0.25) / loupeSpan(0.5)).toBeCloseTo(2);
    expect(loupeSpan(0.1) / loupeSpan(0.25)).toBeCloseTo(2.5);
  });

  it("falls back to a 1:1 view for a missing or invalid scale", () => {
    const fallback = LOUPE_SIZE / LOUPE_ZOOM;
    expect(loupeSpan(0)).toBeCloseTo(fallback);
    expect(loupeSpan(-2)).toBeCloseTo(fallback);
    expect(loupeSpan(Number.NaN)).toBeCloseTo(fallback);
  });

  it("clamps degenerate scales", () => {
    expect(loupeSpan(1e6)).toBe(LOUPE_SPAN_MIN);
    expect(loupeSpan(1e-6)).toBe(LOUPE_SPAN_MAX);
  });
});

describe("loupeTarget", () => {
  it("centers the sampling window on the pointer", () => {
    const target = targetAt(100.2, 50.7);
    expect(target.width).toBe(72);
    expect(target.height).toBe(72);
    expect(target.x).toBeCloseTo(64.2);
    expect(target.y).toBeCloseTo(14.7);
  });

  it("clamps the window at the source edges", () => {
    expect(targetAt(2, 1)).toMatchObject({ x: 0, y: 0 });
    const bottomRight = targetAt(3999.9, 2999.9);
    expect(bottomRight.x).toBe(4000 - SPAN);
    expect(bottomRight.y).toBe(3000 - SPAN);
  });

  it("falls back to the whole image when the source is smaller than the window", () => {
    expect(loupeTarget(10, 10, 40, 30, SPAN)).toEqual({
      x: 0, y: 0, width: 40, height: 30,
    });
  });

  it("rejects empty or non-finite geometry", () => {
    expect(loupeTarget(10, 10, 0, 30, SPAN)).toBeNull();
    expect(loupeTarget(Number.NaN, 10, 40, 30, SPAN)).toBeNull();
  });
});

describe("loupeRegion", () => {
  it("always covers the sampling window across the whole source", () => {
    for (let sourceX = 0; sourceX <= 4000; sourceX += 13) {
      for (const sourceY of [0, 37, 1500, 2999.9]) {
        const target = targetAt(sourceX, sourceY);
        const region = loupeRegion(target, 4000, 3000, SPAN)!;
        expect(region.x).toBeLessThanOrEqual(target.x);
        expect(region.x + region.width).toBeGreaterThanOrEqual(target.x + target.width);
        expect(region.y).toBeLessThanOrEqual(target.y);
        expect(region.y + region.height).toBeGreaterThanOrEqual(target.y + target.height);
        expect(region.maxEdge).toBe(LOUPE_REGION_EDGE);
      }
    }
  });

  it("keeps one cell for small movement and changes it only past a step", () => {
    // 4000x3000、512 瓦片、72 窗口 → 网格步长 440，同一格内移动不换格。
    expect(loupeRegion(targetAt(1000, 750), 4000, 3000, SPAN)).toEqual({
      x: 880, y: 440, width: 512, height: 512, maxEdge: 512,
    });
    expect(loupeRegion(targetAt(1040, 600), 4000, 3000, SPAN)!.x).toBe(880);
    expect(loupeRegion(targetAt(1600, 750), 4000, 3000, SPAN)!.x).toBe(1320);
  });

  it("grows the tile for a large span so the window stays covered", () => {
    // 大画布：跨度按主视图比例放大到 1200，瓦片随之放大到边距后的 1500。
    const span = 1200;
    const target = loupeTarget(4000, 3000, 8000, 6000, span)!;
    const region = loupeRegion(target, 8000, 6000, span)!;
    expect(region.width).toBeGreaterThanOrEqual(span);
    expect(region.x).toBeLessThanOrEqual(target.x);
    expect(region.x + region.width).toBeGreaterThanOrEqual(target.x + target.width);
    expect(region.y + region.height).toBeGreaterThanOrEqual(target.y + target.height);
    // 缩小取样：输出边长取该格在镜片内的显示尺寸（1500 × 216 / 1200 ≈ 270），
    // 而不是源分辨率，避免传输用不到的像素。
    expect(region.maxEdge).toBe(270);
  });

  it("scales the tile output with the device pixel ratio, capped at source resolution", () => {
    const span = 1200;
    const target = loupeTarget(4000, 3000, 8000, 6000, span)!;
    expect(loupeRegion(target, 8000, 6000, span, 2)!.maxEdge).toBe(540);
    // 放大取样：源分辨率 1:1 已是上限，设备像素比再高也不会请求更大的瓦片。
    const zoomed = targetAt(1000, 750);
    expect(loupeRegion(zoomed, 4000, 3000, SPAN, 3)!.maxEdge).toBe(LOUPE_REGION_EDGE);
  });

  it("uses a single cell for a source smaller than the tile", () => {
    const region = loupeRegion(loupeTarget(10, 10, 300, 200, SPAN)!, 300, 200, SPAN)!;
    expect(region).toEqual({ x: 0, y: 0, width: 300, height: 200, maxEdge: 300 });
    expect(loupeNeighbours(region, 300, 200, SPAN)).toEqual([]);
  });
});

describe("loupeNeighbours", () => {
  it("returns the four orthogonal cells", () => {
    const region = loupeRegion(targetAt(1000, 750), 4000, 3000, SPAN)!;
    expect(loupeNeighbours(region, 4000, 3000, SPAN).map((item) => `${item.x},${item.y}`))
      .toEqual(["440,440", "1320,440", "880,0", "880,880"]);
  });
});

describe("loupePlacement", () => {
  it("follows the pointer with a gap when there is room", () => {
    expect(loupePlacement(100, 50, 1000, 600)).toEqual({ left: 100 + LOUPE_GAP, top: 50 + LOUPE_GAP });
  });

  it("flips to the opposite side at the stage edges", () => {
    expect(loupePlacement(900, 500, 1000, 600)).toEqual({
      left: 900 - LOUPE_GAP - LOUPE_SIZE,
      top: 500 - LOUPE_GAP - LOUPE_SIZE,
    });
  });

  it("stays inside a stage smaller than the lens", () => {
    expect(loupePlacement(10, 10, 100, 100)).toEqual({ left: 0, top: 0 });
  });
});

describe("regionKey", () => {
  it("includes the source, the cell geometry and the output edge", () => {
    expect(regionKey("branch-1", loupeRegion(targetAt(1000, 750), 4000, 3000, SPAN)!))
      .toBe("branch-1|880,440,512,512,512");
  });
});

describe("useLoupeFrame", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  const requested = (request: ReturnType<typeof vi.fn>, x: number, y: number) =>
    request.mock.calls.some((call) => {
      const bounds = call[0] as TileRequest;
      return bounds.x === x && bounds.y === y;
    });

  it("fetches only the resting cell and reuses it while the pointer stays inside", async () => {
    const request = vi.fn(async (_bounds: TileRequest) => tile);
    const { rerender } = renderHook(
      ({ pointer }) => useLoupeFrame(frameOptions(pointer, request)),
      { initialProps: { pointer: { x: 0.25, y: 0.25 } } },
    );

    await act(async () => { vi.advanceTimersByTime(LOUPE_FETCH_DEBOUNCE_MS); });
    expect(request).toHaveBeenCalledTimes(1);
    expect(request).toHaveBeenCalledWith({ x: 880, y: 440, width: 512, height: 512, maxEdge: 512 });

    // 同一格内移动指针：不产生任何请求。
    rerender({ pointer: { x: 0.26, y: 0.2 } });
    await act(async () => { vi.advanceTimersByTime(LOUPE_FETCH_DEBOUNCE_MS); });
    expect(request).toHaveBeenCalledTimes(1);

    // 跨过一个步长：只在停留后补新的一块。
    rerender({ pointer: { x: 0.4, y: 0.25 } });
    await act(async () => { vi.advanceTimersByTime(LOUPE_FETCH_DEBOUNCE_MS); });
    expect(request).toHaveBeenCalledTimes(2);
    expect(request).toHaveBeenLastCalledWith({ x: 1320, y: 440, width: 512, height: 512, maxEdge: 512 });

    // 回到已经缓存过的格：不再请求。
    rerender({ pointer: { x: 0.25, y: 0.25 } });
    await act(async () => { vi.advanceTimersByTime(LOUPE_FETCH_DEBOUNCE_MS); });
    expect(request).toHaveBeenCalledTimes(2);
  });

  it("prefetches the neighbouring cells once the pointer settles", async () => {
    const request = vi.fn(async (_bounds: TileRequest) => tile);
    renderHook(() => useLoupeFrame(frameOptions({ x: 0.25, y: 0.25 }, request)));

    await act(async () => { vi.advanceTimersByTime(LOUPE_FETCH_DEBOUNCE_MS); });
    expect(requested(request, 880, 440)).toBe(true);

    await act(async () => { vi.advanceTimersByTime(LOUPE_PREFETCH_DELAY_MS - LOUPE_FETCH_DEBOUNCE_MS); });
    for (const [x, y] of [[440, 440], [1320, 440], [880, 0], [880, 880]]) {
      expect(requested(request, x, y)).toBe(true);
    }
  });

  it("isolates the tile cache per source", async () => {
    const request = vi.fn(async (_bounds: TileRequest) => tile);
    const { rerender } = renderHook(
      ({ sourceKey }) => useLoupeFrame(frameOptions({ x: 0.25, y: 0.25 }, request, sourceKey)),
      { initialProps: { sourceKey: "source-a" } },
    );
    await act(async () => { vi.advanceTimersByTime(LOUPE_FETCH_DEBOUNCE_MS); });
    expect(request).toHaveBeenCalledTimes(1);

    rerender({ sourceKey: "source-b" });
    await act(async () => { vi.advanceTimersByTime(LOUPE_FETCH_DEBOUNCE_MS); });
    expect(request).toHaveBeenCalledTimes(2);
  });

  it("requests nothing without a pointer", async () => {
    const request = vi.fn(async (_bounds: TileRequest) => tile);
    renderHook(() => useLoupeFrame(frameOptions(null, request)));

    await act(async () => { vi.advanceTimersByTime(LOUPE_PREFETCH_DELAY_MS * 3); });
    expect(request).not.toHaveBeenCalled();
  });
});
