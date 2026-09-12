import { describe, expect, it } from "vitest";
import type { PinBoardImage } from "./types";
import {
  arrangeImages,
  fitViewportToBounds,
  imageDisplayMaxDimension,
  imageSize,
  imagesCenter,
  pointInQuad,
  quadIntersectsBounds,
  rotateQuad,
  screenToWorld,
  viewportBounds,
  worldToScreen,
  type Quad,
} from "./geometry";

function image(imageId: number, points: Quad): PinBoardImage {
  return {
    boardId: 1,
    imageId,
    width: 100,
    height: 100,
    order: imageId,
    layer: 1,
    deleted: false,
    points,
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
  };
}

describe("pin-board geometry", () => {
  it("derives viewport width from the canvas aspect ratio", () => {
    expect(viewportBounds({ centerX: 10, centerY: 20, height: 400 }, 800, 400))
      .toEqual({ minX: -390, minY: -180, maxX: 410, maxY: 220 });
  });

  it("round-trips screen and world coordinates", () => {
    const rect = { left: 30, top: 50, width: 900, height: 600 } as DOMRect;
    const viewport = { centerX: 1250, centerY: -340, height: 3000 };
    const world = screenToWorld(660, 230, rect, viewport);
    const screen = worldToScreen(world, rect, viewport);

    expect(screen[0]).toBeCloseTo(630);
    expect(screen[1]).toBeCloseTo(180);
  });

  it("calculates projected texture demand from zoom", () => {
    const source = image(1, [[0, 0], [200, 0], [200, -100], [0, -100]]);
    expect(imageDisplayMaxDimension(source, { centerX: 0, centerY: 0, height: 500 }, 1000))
      .toBe(400);
  });

  it("hits and intersects rotated quads", () => {
    const square: Quad = [[-10, 10], [10, 10], [10, -10], [-10, -10]];
    const rotated = rotateQuad(square, Math.PI / 4, [0, 0]);

    expect(pointInQuad([0, 0], rotated)).toBe(true);
    expect(pointInQuad([20, 20], rotated)).toBe(false);
    expect(quadIntersectsBounds(rotated, { minX: 8, minY: -2, maxX: 16, maxY: 2 }))
      .toBe(true);
  });

  it("arranges images at a common height around the requested center", () => {
    const images = [
      image(1, [[0, 50], [100, 50], [100, 0], [0, 0]]),
      image(2, [[0, 25], [50, 25], [50, 0], [0, 0]]),
    ];
    const arranged = arrangeImages(images, [500, -250], 10);
    const result = images.map((source) => ({
      ...source,
      points: arranged.get(source.imageId) ?? source.points,
    }));

    expect(imageSize(result[0])[1]).toBeCloseTo(50);
    expect(imageSize(result[1])[1]).toBeCloseTo(50);
    expect(imagesCenter(result)[0]).toBeCloseTo(500);
    expect(imagesCenter(result)[1]).toBeCloseTo(-250);
  });

  it("fits the viewport to the minimal bounding box of every image", () => {
    const bounds = { minX: -200, minY: -50, maxX: 1200, maxY: 250 };
    const fitted = fitViewportToBounds(bounds, 1000, 600);
    const view = viewportBounds(fitted, 1000, 600);

    expect(fitted.centerX).toBe(500);
    expect(fitted.centerY).toBe(100);
    // 包围框完整可见，且决定缩放的一侧只多出配置的 CSS 边距。
    expect(view.minX).toBeLessThanOrEqual(bounds.minX);
    expect(view.maxX).toBeGreaterThanOrEqual(bounds.maxX);
    expect(view.minY).toBeLessThanOrEqual(bounds.minY);
    expect(view.maxY).toBeGreaterThanOrEqual(bounds.maxY);
    expect(bounds.minX - view.minX).toBeCloseTo(24 * (view.maxX - view.minX) / 1000, 6);
  });

  it("keeps a degenerate bounding box finite when fitting", () => {
    const fitted = fitViewportToBounds({ minX: 40, minY: 60, maxX: 40, maxY: 60 }, 800, 600);

    expect(Number.isFinite(fitted.height)).toBe(true);
    expect(fitted.height).toBeGreaterThan(0);
    expect(fitted.centerX).toBe(40);
    expect(fitted.centerY).toBe(60);
  });
});
