import { describe, expect, it } from "vitest";
import { formatBytes } from "./format";

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
