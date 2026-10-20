import { describe, expect, it } from "vitest";
import {
  DEFAULT_LAYER_VISIBILITY,
  PIN_BOARD_LAYER_COUNT,
  STROKE_LAYER,
  hiddenSelectionIds,
  isLayerVisible,
  normalizeLayerVisibility,
} from "./layers";

describe("pin-board layer visibility", () => {
  it("covers the four layers including the fixed annotation layer", () => {
    expect(PIN_BOARD_LAYER_COUNT).toBe(4);
    expect(STROKE_LAYER).toBe(3);
    expect(DEFAULT_LAYER_VISIBILITY).toEqual([true, true, true, true]);
  });

  it("normalises missing, partial and non-boolean input to 'shown'", () => {
    expect(normalizeLayerVisibility(null)).toEqual([true, true, true, true]);
    expect(normalizeLayerVisibility(undefined)).toEqual([true, true, true, true]);
    expect(normalizeLayerVisibility([false])).toEqual([false, true, true, true]);
    // 会话里出现非法值时宁可显示，也不要把一层内容永久藏起来。
    expect(normalizeLayerVisibility([false, undefined as unknown as boolean, true, false]))
      .toEqual([false, true, true, false]);
  });

  it("returns a fresh tuple so callers cannot mutate the default", () => {
    const first = normalizeLayerVisibility(DEFAULT_LAYER_VISIBILITY);
    first[0] = false;

    expect(DEFAULT_LAYER_VISIBILITY[0]).toBe(true);
    expect(normalizeLayerVisibility(DEFAULT_LAYER_VISIBILITY)[0]).toBe(true);
  });

  it("treats unknown layers as visible", () => {
    const hidden = normalizeLayerVisibility([false, false, false, false]);

    expect(isLayerVisible(hidden, 0)).toBe(false);
    expect(isLayerVisible(hidden, 9)).toBe(true);
  });

  it("collects the selected items whose layer just got hidden", () => {
    const items = [
      { itemId: 1, layer: 0 },
      { itemId: 2, layer: 1 },
      { itemId: 3, layer: 3 },
    ];

    expect(hiddenSelectionIds([1, 2, 3], items, [true, false, true, false])).toEqual([2, 3]);
    expect(hiddenSelectionIds([1], items, [true, true, true, true])).toEqual([]);
  });

  it("ignores selection ids that are no longer on the board", () => {
    expect(hiddenSelectionIds([7], [{ itemId: 1, layer: 0 }], [false, true, true, true]))
      .toEqual([]);
  });
});
