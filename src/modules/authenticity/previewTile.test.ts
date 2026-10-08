import { describe, expect, it } from "vitest";
import type { PreviewViewport } from "./previewViewport";
import { tileCacheKey, tileOverlayStyle, tileRequestForView } from "./previewTile";

const viewport = (overrides: Partial<PreviewViewport> = {}): PreviewViewport => ({
  scrollLeft: 0,
  scrollTop: 0,
  scrollWidth: 1000,
  scrollHeight: 800,
  clientWidth: 1000,
  clientHeight: 800,
  ...overrides,
});

const base = {
  thumbWidth: 1000,
  thumbHeight: 800,
  sourceWidth: 4000,
  sourceHeight: 3200,
};

describe("tileRequestForView", () => {
  it("skips the request when the thumbnail already carries source resolution", () => {
    expect(tileRequestForView({
      ...base,
      sourceWidth: 1000,
      sourceHeight: 800,
      displayWidth: 4000,
      displayHeight: 3200,
      viewport: viewport(),
    })).toBeNull();
  });

  it("skips the request while the thumbnail is not enlarged", () => {
    // 显示尺寸未超过缩略图：直接显示缩略图就是 1:1 或降采样，局部不会更清晰。
    expect(tileRequestForView({
      ...base,
      displayWidth: 1000,
      displayHeight: 800,
      viewport: viewport(),
    })).toBeNull();
    expect(tileRequestForView({
      ...base,
      displayWidth: 900,
      displayHeight: 720,
      viewport: viewport(),
    })).toBeNull();
  });

  it("requires a viewport before requesting", () => {
    expect(tileRequestForView({
      ...base,
      displayWidth: 2000,
      displayHeight: 1600,
      viewport: null,
    })).toBeNull();
  });

  it("covers the visible region with margin in source coordinates", () => {
    const request = tileRequestForView({
      ...base,
      displayWidth: 2000,
      displayHeight: 1600,
      viewport: viewport(),
    });
    // 显示 2000x1600 对应源图 4000x3200：每个显示像素 = 2 源像素。可视 1000x800
    // 显示像素 → 2000x1600 源像素；起点吸附到 0，终点加 25% 边距并吸附到格宽。
    expect(request).toEqual({ x: 0, y: 0, width: 2500, height: 2000, maxEdge: 1250 });
  });

  it("sizes the tile output to its on-screen footprint", () => {
    const request = tileRequestForView({
      ...base,
      displayWidth: 2000,
      displayHeight: 1600,
      viewport: viewport(),
    })!;
    // 局部在屏幕上按 width / scaleX x height / scaleY 呈现；输出边长与之相等才能 1:1。
    const displayedWidth = request.width / base.sourceWidth * 2000;
    const displayedHeight = request.height / base.sourceHeight * 1600;
    expect(Math.max(displayedWidth, displayedHeight)).toBe(request.maxEdge);
  });

  it("covers the bottom-right corner when scrolled there", () => {
    const request = tileRequestForView({
      ...base,
      displayWidth: 2000,
      displayHeight: 1600,
      viewport: viewport({
        clientWidth: 500,
        clientHeight: 400,
        scrollLeft: 1500,
        scrollTop: 1200,
        scrollWidth: 2000,
        scrollHeight: 1600,
      }),
    });
    expect(request).not.toBeNull();
    // 可视区域右下角必须覆盖到源图右下角。
    expect(request!.x + request!.width).toBe(4000);
    expect(request!.y + request!.height).toBe(3200);
    expect(request!.maxEdge).toBe(625);
  });

  it("reuses one rect for small pans", () => {
    const at = (scrollLeft: number) => tileRequestForView({
      ...base,
      displayWidth: 2000,
      displayHeight: 1600,
      viewport: viewport({
        clientWidth: 500,
        clientHeight: 400,
        scrollLeft,
        scrollTop: 1200,
        scrollWidth: 2000,
        scrollHeight: 1600,
      }),
    });
    // 吸附格宽是可视跨度的 25%（此处 250 源像素），小幅平移落在同一格内，不重新请求。
    expect(at(1450)).toEqual(at(1400));
  });

  it("ignores empty visible areas", () => {
    expect(tileRequestForView({
      ...base,
      displayWidth: 2000,
      displayHeight: 1600,
      viewport: viewport({ clientWidth: 0, clientHeight: 0 }),
    })).toBeNull();
  });
});

describe("tileCacheKey", () => {
  it("distinguishes position, size and resolution", () => {
    const request = { x: 10, y: 20, width: 30, height: 40, maxEdge: 256 };
    expect(tileCacheKey(request)).toBe("10,20,30,40,256");
    expect(tileCacheKey({ ...request, x: 11 })).not.toBe(tileCacheKey(request));
    expect(tileCacheKey({ ...request, maxEdge: 512 })).not.toBe(tileCacheKey(request));
  });
});

describe("tileOverlayStyle", () => {
  it("maps the source rect to fractions of the displayed image", () => {
    const style = tileOverlayStyle({ x: 1000, y: 800, width: 2000, height: 1600, maxEdge: 512 }, 4000, 3200);
    expect(style).toEqual({ left: "25%", top: "25%", width: "50%", height: "50%" });
  });
});
