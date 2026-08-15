import { describe, expect, it } from "vitest";
import { navigatorRect, navigatorScrollTarget, type PreviewViewport } from "./previewViewport";

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
