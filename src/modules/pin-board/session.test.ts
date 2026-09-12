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
    });

    expect(getBoardSession("artwork-c", 2)).toBeNull();
    expect(getBoardSession("artwork-d", 1)).toBeNull();
    expect(getBoardSession("artwork-c", 1)?.selectedImageIds).toEqual([7]);
  });

  it("returns defensive copies of selected image ids", () => {
    setBoardSession("artwork-e", 5, {
      centerX: 1,
      centerY: 2,
      worldUnitsPerCssPixel: 4,
      locked: false,
      selectedImageIds: [8, 9],
    });
    const session = getBoardSession("artwork-e", 5);
    session?.selectedImageIds.push(10);

    expect(getBoardSession("artwork-e", 5)?.selectedImageIds).toEqual([8, 9]);
  });

  it("drops sessions when the artwork workspace unmounts", async () => {
    setSelectedBoardId("artwork-f", 3);
    const { dropArtworkSessions } = await import("./session");
    dropArtworkSessions("artwork-f");

    expect(getSelectedBoardId("artwork-f")).toBeNull();
  });
});
