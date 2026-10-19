import { describe, expect, it } from "vitest";
import {
  hexToHsv,
  hsvToHex,
  normaliseHexColor,
} from "./PinBoardColorPicker";

describe("pin-board colour picker", () => {
  it("normalises short and long hex values", () => {
    expect(normaliseHexColor("#774B4B")).toBe("#774b4b");
    expect(normaliseHexColor("774b4b")).toBe("#774b4b");
    expect(normaliseHexColor("#abc")).toBe("#aabbcc");
    expect(normaliseHexColor("#12345")).toBeNull();
    expect(normaliseHexColor("not-a-colour")).toBeNull();
  });

  it("maps HSV primaries and greys", () => {
    expect(hsvToHex({ hue: 0, saturation: 1, value: 1 })).toBe("#ff0000");
    expect(hsvToHex({ hue: 120, saturation: 1, value: 1 })).toBe("#00ff00");
    expect(hsvToHex({ hue: 240, saturation: 1, value: 1 })).toBe("#0000ff");
    expect(hsvToHex({ hue: 0, saturation: 0, value: 0 })).toBe("#000000");
    expect(hsvToHex({ hue: 0, saturation: 0, value: 1 })).toBe("#ffffff");
  });

  it("round-trips hex colours through HSV", () => {
    for (const hex of [
      "#000000",
      "#ffffff",
      "#3f3f46",
      "#a1a1aa",
      "#774b4b",
      "#dc2626",
      "#16a34a",
      "#2563eb",
      "#7c3aed",
    ]) {
      const hsv = hexToHsv(hex);
      expect(hsv).not.toBeNull();
      expect(hsvToHex(hsv!)).toBe(hex);
    }
  });
});
