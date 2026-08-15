import { describe, expect, it } from "vitest";
import {
  clampPreviewZoom, navigatorRect, navigatorScrollTarget, previewZoomFromButton,
  previewZoomFromWheel, type PreviewViewport, zoomAnchorScrollTarget,
} from "./previewViewport";

const viewport: PreviewViewport = {
  scrollLeft: 500,
  scrollTop: 250,
  scrollWidth: 2000,
  scrollHeight: 1000,
  clientWidth: 500,
  clientHeight: 250,
};

describe("preview navigator", () => {
  it("maps the visible canvas to a normalized rectangle", () => {
    expect(navigatorRect(viewport)).toEqual({ left: 25, top: 25, width: 25, height: 25 });
  });

  it("centers and clamps navigator drag targets", () => {
    expect(navigatorScrollTarget(viewport, 1, 1)).toEqual({ scrollLeft: 1500, scrollTop: 750 });
    expect(navigatorScrollTarget(viewport, 0, 0)).toEqual({ scrollLeft: 0, scrollTop: 0 });
  });
});

describe("preview zoom", () => {
  it("starts wheel zoom from the actual fitted scale", () => {
    expect(previewZoomFromWheel(0.2, -100, 0.2)).toBeCloseTo(0.221, 3);
    expect(previewZoomFromWheel(0.2, 100, 0.2)).toBeCloseTo(0.181, 3);
  });

  it("preserves fine trackpad deltas and bounds large wheel deltas", () => {
    expect(previewZoomFromWheel(1, -1, 0.25)).toBeCloseTo(1.001, 3);
    expect(previewZoomFromWheel(1, -1000, 0.25)).toBeCloseTo(previewZoomFromWheel(1, -100, 0.25), 8);
  });

  it("uses ten-percent toolbar steps and keeps a small fitted image reachable", () => {
    expect(previewZoomFromButton(1, 1, 0.25)).toBeCloseTo(1.1, 8);
    expect(previewZoomFromButton(1, -1, 0.25)).toBeCloseTo(1 / 1.1, 8);
    expect(clampPreviewZoom(0, 0.02)).toBe(0.02);
    expect(clampPreviewZoom(10, 0.25)).toBe(4);
  });

  it("keeps the image point under the pointer fixed while zooming", () => {
    expect(zoomAnchorScrollTarget(
      { xRatio: 0.25, yRatio: 0.75, canvasX: 300, canvasY: 180 },
      40,
      20,
      2400,
      1600,
    )).toEqual({ scrollLeft: 340, scrollTop: 1040 });
  });
});
