import { describe, expect, it } from "vitest";
import { reorderBoardIds } from "./boardOrder";

const order = [1, 2, 3, 4];

describe("board drag reorder", () => {
  it("moves a board before or after the drop target", () => {
    expect(reorderBoardIds(order, 4, 1, "before")).toEqual([4, 1, 2, 3]);
    expect(reorderBoardIds(order, 4, 1, "after")).toEqual([1, 4, 2, 3]);
    expect(reorderBoardIds(order, 1, 4, "after")).toEqual([2, 3, 4, 1]);
    expect(reorderBoardIds(order, 1, 3, "before")).toEqual([2, 1, 3, 4]);
  });

  it("returns null when the order would not change", () => {
    expect(reorderBoardIds(order, 2, 2, "before")).toBeNull();
    expect(reorderBoardIds(order, 2, 1, "after")).toBeNull();
    expect(reorderBoardIds(order, 1, 2, "before")).toBeNull();
  });

  it("returns null for ids outside the list", () => {
    expect(reorderBoardIds(order, 9, 1, "before")).toBeNull();
    expect(reorderBoardIds(order, 1, 9, "after")).toBeNull();
  });

  it("never drops a board by changing which ids are present", () => {
    const next = reorderBoardIds(order, 3, 1, "before");
    expect(next).not.toBeNull();
    expect([...next!].sort((left, right) => left - right)).toEqual([1, 2, 3, 4]);
  });
});
