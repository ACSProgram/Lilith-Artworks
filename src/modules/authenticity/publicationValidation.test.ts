import { describe, expect, it } from "vitest";
import type { CertificationConfig } from "./types";
import { publicationPreviewError, publicationPreviewSignature } from "./publicationValidation";

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
