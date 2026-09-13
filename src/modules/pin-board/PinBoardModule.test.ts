import { describe, expect, it } from "vitest";
import {
  PIN_BOARD_FULLSCREEN_SHORTCUT,
  PIN_BOARD_LOCK_SHORTCUT,
  reorderBoardIds,
} from "./PinBoardModule";
import { shortcutMatches } from "./shortcuts";

describe("pin-board module entry", () => {
  it("loads through its public React entry without starting native work", async () => {
    const module = await import("./PinBoardModule");
    expect(module.PinBoardModule).toBeTypeOf("function");
  });
});

describe("pin-board default shortcuts", () => {
  it("keeps the Client default Ctrl+R for the lock action", () => {
    // 锁定沿用 Client 的默认键位；WebView 刷新由应用层只取消默认行为来屏蔽，
    // 因此 Ctrl+R 必须仍然能命中锁定快捷键。
    expect(PIN_BOARD_LOCK_SHORTCUT).toBe("CommandOrControl+R");
    expect(shortcutMatches(
      { key: "r", code: "KeyR", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false } as KeyboardEvent,
      PIN_BOARD_LOCK_SHORTCUT,
    )).toBe(true);
    expect(PIN_BOARD_FULLSCREEN_SHORTCUT).toBe("F11");
  });
});

describe("board drag reorder", () => {
  const order = [1, 2, 3, 4];

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
