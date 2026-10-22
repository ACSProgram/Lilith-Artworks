import { describe, expect, it } from "vitest";
import { fileName, formatBytes } from "./format";

describe("fileName", () => {
  it("keeps only the last path segment for either separator style", () => {
    expect(fileName("C:\\artworks\\final.png")).toBe("final.png");
    expect(fileName("/home/user/final.png")).toBe("final.png");
  });

  it("falls back to the input when no separator is present", () => {
    expect(fileName("final.png")).toBe("final.png");
    expect(fileName("")).toBe("");
  });
});

describe("formatBytes", () => {
  it("formats binary units with stable precision", () => {
    expect(formatBytes(1536)).toBe("1.50 KiB");
    expect(formatBytes(12 * 1024)).toBe("12.0 KiB");
  });

  it("rejects invalid sizes", () => {
    expect(formatBytes(-1)).toBe("-");
    expect(formatBytes(Number.NaN)).toBe("-");
  });
});
