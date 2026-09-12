import { describe, expect, it } from "vitest";
import {
  bc7TextureDimensions,
  bc7TextureUvScale,
  PIN_BOARD_NATIVE_TEXTURE_LIMIT,
  PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES,
  estimatedTextureLoadBytes,
  estimatedTextureResidentBytes,
  previewDimensions,
  requestedTextureDimension,
  rgbaPreviewBytes,
  rgbaPreviewDimensionLimit,
  textureBudgetsForLevel,
  textureUploadLayout,
} from "./texturePolicy";

describe("pin-board texture policy", () => {
  it("maps cache levels to resident preview budgets", () => {
    expect(textureBudgetsForLevel("low")).toEqual({
      previewBytes: 64 * 1024 * 1024,
      residentBytes: 128 * 1024 * 1024,
    });
    expect(textureBudgetsForLevel("medium")).toEqual({
      previewBytes: 128 * 1024 * 1024,
      residentBytes: 256 * 1024 * 1024,
    });
    expect(textureBudgetsForLevel("high")).toEqual({
      previewBytes: 256 * 1024 * 1024,
      residentBytes: 512 * 1024 * 1024,
    });
    expect(textureBudgetsForLevel(undefined)).toEqual(textureBudgetsForLevel("medium"));
  });

  it("pads BC7 extents to the physical 4x4 block grid", () => {
    expect(bc7TextureDimensions(4646, 6970)).toEqual([4648, 6972]);
    expect(bc7TextureDimensions(3087, 4346)).toEqual([3088, 4348]);
    expect(bc7TextureDimensions(7000, 7000)).toEqual([7000, 7000]);
  });

  it("scales UVs so padded BC7 edge blocks stay outside the image", () => {
    expect(bc7TextureUvScale(4646, 6970)).toEqual([
      4646 / 4648,
      6970 / 6972,
    ]);
    expect(bc7TextureUvScale(7000, 7000)).toEqual([1, 1]);
  });

  it("matches the native aspect-preserving preview dimensions", () => {
    expect(previewDimensions({ width: 12_000, height: 6_000 }, 2048))
      .toEqual([2048, 1024]);
    expect(previewDimensions({ width: 6_000, height: 12_000 }, 2048))
      .toEqual([1024, 2048]);
  });

  it("bounds a square RGBA preview to 64 MiB", () => {
    const source = { width: 16_384, height: 16_384 };
    const limit = rgbaPreviewDimensionLimit(source);

    expect(limit).toBe(4096);
    expect(rgbaPreviewBytes(source, limit)).toBe(PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES);
  });

  it("uses the same budget efficiently for wide images", () => {
    const source = { width: 32_768, height: 8192 };
    const limit = rgbaPreviewDimensionLimit(source);

    expect(limit).toBe(8192);
    expect(previewDimensions(source, limit)).toEqual([8192, 2048]);
    expect(rgbaPreviewBytes(source, limit)).toBe(PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES);
  });

  it("never requests beyond the native limit at high zoom", () => {
    const requested = requestedTextureDimension(
      { width: 16_384, height: 16_384 },
      100_000,
      16_384,
    );

    expect(requested).toBe(4096);
    expect(requested).toBeLessThanOrEqual(PIN_BOARD_NATIVE_TEXTURE_LIMIT);
  });

  it("loads a full BC7 source once it is useful and supported", () => {
    const source = { width: 8000, height: 8000 };

    expect(requestedTextureDimension(source, 3199, 16_384)).toBe(3999);
    expect(requestedTextureDimension(source, 3200, 16_384)).toBe(8000);
  });

  it("saturates large-source requests instead of re-requesting an unattainable size", () => {
    const source = { width: 20_000, height: 20_000 };
    const requests = [1000, 4000, 8000, 16_000, 32_000]
      .map((display) => requestedTextureDimension(source, display, 16_384));

    expect(requests).toEqual([1250, 4096, 4096, 4096, 4096]);
  });

  it("keeps load estimates conservative for previews and source DDS payloads", () => {
    const previewEstimate = estimatedTextureLoadBytes(
      { width: 16_384, height: 16_384 },
      4096,
    );
    const sourceEstimate = estimatedTextureLoadBytes(
      { width: 4096, height: 4096 },
      4096,
    );

    expect(previewEstimate).toBeGreaterThan(PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES);
    expect(sourceEstimate).toBeGreaterThan(16 * 1024 * 1024);
  });

  it("pads non-aligned RGBA rows without changing their pixel bytes", () => {
    const source = Uint8Array.from([1, 2, 3, 4, 5, 6]);
    const upload = textureUploadLayout(source, 3, 2);

    expect(upload.bytesPerRow).toBe(256);
    expect(upload.rowsPerImage).toBe(2);
    expect([...upload.data.subarray(0, 3)]).toEqual([1, 2, 3]);
    expect([...upload.data.subarray(256, 259)]).toEqual([4, 5, 6]);
    expect(upload.data.byteLength).toBe(512);
  });

  it("reuses already aligned compressed rows", () => {
    const source = new Uint8Array(512);
    const upload = textureUploadLayout(source, 256, 2);

    expect(upload.data).toBe(source);
    expect(upload.bytesPerRow).toBe(256);
  });

  it("estimates the aligned resident size used by the eviction budget", () => {
    expect(estimatedTextureResidentBytes({ width: 7000, height: 7000 }, 7000))
      .toBe(49_280_000);
    expect(estimatedTextureResidentBytes({ width: 16_384, height: 16_384 }, 4096))
      .toBe(67_108_864);
  });
});
