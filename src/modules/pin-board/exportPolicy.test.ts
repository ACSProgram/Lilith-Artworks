import { describe, expect, it } from "vitest";
import {
  CANVAS_EXPORT_BAND_MAX_HEIGHT,
  CANVAS_EXPORT_BAND_MIN_HEIGHT,
  CANVAS_EXPORT_DEFAULT_MARGIN,
  CANVAS_EXPORT_HARD_MAX_DIMENSION,
  CANVAS_EXPORT_MAX_MARGIN,
  CANVAS_EXPORT_MAX_SCALE,
  CANVAS_EXPORT_TILE_MAX_WIDTH,
  autoCanvasExportScale,
  canvasExportBandHeight,
  canvasExportMaxDimension,
  canvasExportTiles,
  resolveCanvasExportPlan,
  resolveCanvasExportScale,
} from "./exportPolicy";

const bounds = (width: number, height: number) => ({
  minX: 0,
  minY: 0,
  maxX: width,
  maxY: height,
});

describe("canvas export scale", () => {
  it("keeps the requested factor even when the limit would suggest less", () => {
    // 手动倍率优先：上限 20000 只提示，不再压低 4x。
    const size = resolveCanvasExportScale(bounds(10_000, 5_600), 4, 0, 20_000);
    expect(size?.scale).toBe(4);
    expect(size?.width).toBe(40_000);
    expect(size?.height).toBe(22_400);
    expect(size?.overLimit).toBe(true);
  });

  it("reports the limit only through the longest edge", () => {
    const exact = resolveCanvasExportScale(bounds(10_000, 5_600), 2, 0, 20_000);
    expect(exact?.width).toBe(20_000);
    expect(exact?.height).toBe(11_200);
    expect(exact?.overLimit).toBe(false);

    const over = resolveCanvasExportScale(bounds(10_000, 5_600), 3, 0, 20_000);
    expect(over?.overLimit).toBe(true);
  });

  it("counts the margin into the longest edge", () => {
    const size = resolveCanvasExportScale(bounds(10_000, 5_000), 1, 64, 20_000);
    expect(size?.width).toBe(10_128);
    expect(size?.height).toBe(5_128);
    expect(size?.overLimit).toBe(false);
  });

  it("clamps the margin to the shared range", () => {
    const size = resolveCanvasExportScale(bounds(1000, 1000), 1, 5_000, 20_000);
    expect(size?.width).toBe(1000 + CANVAS_EXPORT_MAX_MARGIN * 2);
    expect(size?.height).toBe(1000 + CANVAS_EXPORT_MAX_MARGIN * 2);
  });

  it("converges absurd factors to the hard caps", () => {
    // 10000 宽填 99x：先撞边长 65535，倍率收敛到 6.5535，宽度正好是硬上限。
    const byDimension = resolveCanvasExportScale(bounds(10_000, 1_000), 99, 0, 20_000);
    expect(byDimension?.width).toBe(CANVAS_EXPORT_HARD_MAX_DIMENSION);
    expect(byDimension!.scale).toBeCloseTo(
      CANVAS_EXPORT_HARD_MAX_DIMENSION / 10_000,
      10,
    );

    // 小画板填 99x：撞的是倍率上限 8x。
    const byScale = resolveCanvasExportScale(bounds(100, 80), 99, 0, 20_000);
    expect(byScale?.scale).toBe(CANVAS_EXPORT_MAX_SCALE);
    expect(byScale?.width).toBe(800);
  });

  it("rejects degenerate content and invalid factors", () => {
    expect(resolveCanvasExportScale(bounds(0, 0), 2, 0, 20_000)).toBeNull();
    expect(resolveCanvasExportScale(bounds(100, 100), 0, 0, 20_000)).toBeNull();
    expect(resolveCanvasExportScale(bounds(100, 100), Number.NaN, 0, 20_000)).toBeNull();
  });

  it("never overshoots the pixel cap after rounding", () => {
    // √10 亿见方的内容：两边各自向上取整就多出十几万像素，原生侧按同一个数字会拒绝，
    // 因此取整后必须再收敛一次。
    const side = Math.sqrt(1_000_000_000);
    const size = resolveCanvasExportScale(bounds(side, side), 1, 0, 30_000);
    expect(size!.width * size!.height).toBeLessThanOrEqual(1_000_000_000);
    // 返回的倍率与最终尺寸一致：渲染按它把块坐标映射回世界坐标。
    expect(size!.scale).toBeCloseTo(size!.width / side, 10);
  });
});

describe("canvas export defaults", () => {
  it("derives the default factor from the suggestion limit", () => {
    // 45000 宽塞进 20000 → 0.4444，向下取两位 → 0.44。
    expect(autoCanvasExportScale(bounds(45_000, 30_000), 0, 20_000)).toBe(0.44);
    // 内容装得下就是 1x，不会给出 2x 这种「自动放大」。
    expect(autoCanvasExportScale(bounds(1_000, 800), 0, 20_000)).toBe(1);
    // 边距算进最长边。
    expect(autoCanvasExportScale(bounds(19_990, 1_000), CANVAS_EXPORT_DEFAULT_MARGIN, 20_000))
      .toBe(0.99);
  });

  it("falls back to 1x when the content is empty", () => {
    expect(autoCanvasExportScale(bounds(0, 0), 10, 20_000)).toBe(1);
  });
});

describe("canvas export bands", () => {
  it("derives the band height from the row bytes and clamps both ends", () => {
    // 32 MiB ÷ (宽 × 4)：8000 → 1048，20000 → 419，30000 → 279。
    expect(canvasExportBandHeight(8_000)).toBe(1048);
    expect(canvasExportBandHeight(20_000)).toBe(419);
    expect(canvasExportBandHeight(30_000)).toBe(279);
    // 极窄画板会算出极大的行带，夹到上限；极宽则夹到下限。
    expect(canvasExportBandHeight(100)).toBe(CANVAS_EXPORT_BAND_MAX_HEIGHT);
    expect(canvasExportBandHeight(200_000)).toBe(CANVAS_EXPORT_BAND_MIN_HEIGHT);
  });

  it("splits the output into band-major tiles with the remainder in the last one", () => {
    const plan = resolveCanvasExportPlan(bounds(10_000, 5_000), 2, 0, 20_000);
    expect(plan).not.toBeNull();
    // 20000 宽 → 3 块（8192 + 8192 + 3616），行带 419 → 10000 高 = 24 条行带。
    expect(plan!.tileWidth).toBe(CANVAS_EXPORT_TILE_MAX_WIDTH);
    expect(plan!.tilesPerBand).toBe(3);
    expect(plan!.bandHeight).toBe(419);
    expect(plan!.bandCount).toBe(Math.ceil(10_000 / 419));
    expect(plan!.totalTiles).toBe(plan!.tilesPerBand * plan!.bandCount);
  });

  it("emits every tile in row-major order with exact edges", () => {
    const plan = resolveCanvasExportPlan(bounds(9_000, 300), 1, 0, 20_000);
    expect(plan).not.toBeNull();
    const tiles = canvasExportTiles(plan!);
    expect(tiles).toHaveLength(plan!.totalTiles);
    // 行带优先：第一块从原点开始，同一行带内 x 递增。
    expect(tiles[0]).toEqual({ x: 0, y: 0, width: 8192, height: plan!.bandHeight });
    expect(tiles[1].x).toBe(8192);
    expect(tiles[1].width).toBe(9000 - 8192);
    expect(tiles[1].y).toBe(0);
    // 最后一块正好补满右侧，不越界。
    const last = tiles[tiles.length - 1];
    expect(last.x + last.width).toBe(plan!.width);
    expect(last.y + last.height).toBe(plan!.height);
    for (const tile of tiles) {
      expect(tile.x + tile.width).toBeLessThanOrEqual(plan!.width);
      expect(tile.y + tile.height).toBeLessThanOrEqual(plan!.height);
    }
  });

  it("honours a device tile limit lower than 8192", () => {
    const plan = resolveCanvasExportPlan(bounds(9_000, 300), 1, 0, 20_000, 4096);
    expect(plan?.tileWidth).toBe(4096);
    expect(plan?.tilesPerBand).toBe(3);
  });

  it("returns null when there is nothing to export", () => {
    expect(resolveCanvasExportPlan(bounds(0, 0), 2, 0, 20_000)).toBeNull();
  });

  it("carries the suggested limit and the over-limit flag into the plan", () => {
    const plan = resolveCanvasExportPlan(bounds(10_000, 5_600), 4, 0, 20_000);
    expect(plan!.limit).toBe(20_000);
    expect(plan!.requestedScale).toBe(4);
    expect(plan!.overLimit).toBe(true);
    expect(plan!.margin).toBe(0);
  });
});

describe("canvas export limits", () => {
  it("maps each texture cache level to its own longest edge", () => {
    expect(canvasExportMaxDimension("low")).toBe(10_000);
    expect(canvasExportMaxDimension("medium")).toBe(20_000);
    expect(canvasExportMaxDimension("high")).toBe(30_000);
  });
});
