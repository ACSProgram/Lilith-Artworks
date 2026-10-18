import { describe, expect, it } from "vitest";
import type { CertificationConfig, NormalizedRegion } from "./types";
import {
  publicationPreviewError, publicationPreviewSignature, regionPixelBounds, watermarkSizeWarnings,
} from "./publicationValidation";

const config = (overrides: Partial<CertificationConfig> = {}): CertificationConfig => ({
  branchId: "branch",
  title: "Artwork",
  creator: "Creator",
  rightsStatement: "",
  authenticationContent: "",
  trustmarkEnabled: false,
  certificatePath: "C:/certificate.pem",
  signingAlgorithm: "es256",
  timestampUrl: null,
  jpegQuality: 90,
  backgroundColor: "#ffffff",
  watermarkStrength: 1,
  additionalRegions: [],
  updatedMs: 0,
  ...overrides,
});

describe("publicationPreviewError", () => {
  it("requires the private key before generating pixels", () => {
    expect(publicationPreviewError(config(), "   ")).toContain("PEM 私钥");
  });

  it("accepts complete publication inputs", () => {
    expect(publicationPreviewError(config(), "private-key")).toBeNull();
  });
});

describe("publicationPreviewSignature", () => {
  it("invalidates a preview when a pixel-affecting setting changes", () => {
    const initial = publicationPreviewSignature(config(), "mark");
    const changed = publicationPreviewSignature(config({ jpegQuality: 60 }), "mark");

    expect(changed).not.toBe(initial);
  });

  it("does not invalidate pixels for metadata-only changes", () => {
    const initial = publicationPreviewSignature(config(), "mark");
    const changed = publicationPreviewSignature(config({ title: "Renamed" }), "mark");

    expect(changed).toBe(initial);
  });
});

describe("regionPixelBounds", () => {
  it("matches the backend floor/ceil pixel conversion", () => {
    const region: NormalizedRegion = { x: 0, y: 0, width: 0.1875, height: 0.5 };

    expect(regionPixelBounds(region, 512, 512)).toEqual({ width: 96, height: 256 });
  });
});

describe("watermarkSizeWarnings", () => {
  // 与后端同口径：源像素尺寸即 rendition 像素尺寸，起点 floor、终点 ceil。
  const edge = 512;
  const region = (width: number, x = 0): NormalizedRegion => ({
    x,
    y: 0,
    width: width / edge,
    height: 256 / edge,
  });

  it("warns only for regions whose shorter edge is under the stable threshold", () => {
    const warnings = watermarkSizeWarnings(
      [region(96), region(128, 0.25), region(192, 0.5), region(256), region(512)],
      edge,
      edge,
    );

    expect(warnings).toEqual([
      { regionIndex: 0, width: 96, height: 256, threshold: 256 },
      { regionIndex: 1, width: 128, height: 256, threshold: 256 },
      { regionIndex: 2, width: 192, height: 256, threshold: 256 },
    ]);
  });

  it("returns no warnings when every region is at or above the stable edge", () => {
    const warnings = watermarkSizeWarnings([region(256), region(512)], edge, edge);

    expect(warnings).toEqual([]);
  });
});
