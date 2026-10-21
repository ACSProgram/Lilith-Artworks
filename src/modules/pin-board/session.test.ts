import { describe, expect, it } from "vitest";
import {
  getBoardSession,
  getSelectedBoardId,
  setBoardSession,
  setSelectedBoardId,
} from "./session";

describe("pin-board process sessions", () => {
  it("isolates board selections by artwork", () => {
    setSelectedBoardId("artwork-a", 12);
    setSelectedBoardId("artwork-b", 34);

    expect(getSelectedBoardId("artwork-a")).toBe(12);
    expect(getSelectedBoardId("artwork-b")).toBe(34);
    expect(getSelectedBoardId("artwork-c")).toBeNull();
  });

  it("isolates view state by artwork and board", () => {
    setBoardSession("artwork-c", 1, {
      centerX: 10,
      centerY: 20,
      worldUnitsPerCssPixel: 3,
      locked: false,
      selectedImageIds: [7],
      visibleLayers: [true, true, false, true],
    });

    expect(getBoardSession("artwork-c", 2)).toBeNull();
    expect(getBoardSession("artwork-d", 1)).toBeNull();
    expect(getBoardSession("artwork-c", 1)?.selectedImageIds).toEqual([7]);
    expect(getBoardSession("artwork-c", 1)?.visibleLayers).toEqual([true, true, false, true]);
  });

  it("returns defensive copies of selected image ids", () => {
    setBoardSession("artwork-e", 5, {
      centerX: 1,
      centerY: 2,
      worldUnitsPerCssPixel: 4,
      locked: false,
      selectedImageIds: [8, 9],
      visibleLayers: [true, true, true, true],
    });
    const session = getBoardSession("artwork-e", 5);
    session?.selectedImageIds.push(10);
    if (session) session.visibleLayers[0] = false;

    expect(getBoardSession("artwork-e", 5)?.selectedImageIds).toEqual([8, 9]);
    expect(getBoardSession("artwork-e", 5)?.visibleLayers).toEqual([true, true, true, true]);
  });
});
