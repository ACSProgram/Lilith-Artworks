import { describe, expect, it } from "vitest";
import { MENU_VIEWPORT_MARGIN, placeMenu } from "./menuPlacement";

const viewport = { width: 1000, height: 800 };

describe("placeMenu", () => {
  it("keeps the pointer anchor when the menu fits", () => {
    expect(placeMenu({ x: 300, y: 200 }, { width: 230, height: 400 }, viewport))
      .toEqual({ left: 300, top: 200 });
  });

  it("pulls the menu back from the right and bottom edges", () => {
    expect(placeMenu({ x: 980, y: 790 }, { width: 230, height: 400 }, viewport))
      .toEqual({ left: viewport.width - 230 - MENU_VIEWPORT_MARGIN, top: viewport.height - 400 - MENU_VIEWPORT_MARGIN });
  });

  it("keeps the menu inside the viewport on the left and top edges", () => {
    expect(placeMenu({ x: -20, y: 0 }, { width: 230, height: 400 }, viewport))
      .toEqual({ left: MENU_VIEWPORT_MARGIN, top: MENU_VIEWPORT_MARGIN });
  });

  it("falls back to the margin when the menu is larger than the viewport", () => {
    expect(placeMenu({ x: 500, y: 400 }, { width: 1400, height: 900 }, viewport))
      .toEqual({ left: MENU_VIEWPORT_MARGIN, top: MENU_VIEWPORT_MARGIN });
  });
});
