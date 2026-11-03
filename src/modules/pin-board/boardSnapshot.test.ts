import { describe, expect, it } from "vitest";
import {
  boardStateKey,
  cloneBoardItem,
  editableBoardImages,
  restoreSnapshots,
  snapshotImages,
} from "./boardSnapshot";
import { pinBoardTextSignature } from "./textEditing";
import type { PinBoardItem, PinBoardTextItem } from "./types";

function textItem(overrides: Partial<PinBoardTextItem> = {}): PinBoardTextItem {
  return {
    boardId: 1,
    itemId: 7,
    kind: "text",
    width: 40,
    height: 26,
    order: 7,
    layer: 1,
    deleted: false,
    points: [[0, 0], [40, 0], [40, 52], [0, 52]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
    text: "ab",
    fontFamily: "sans-serif",
    fontSize: 20,
    color: "#112233",
    align: "center",
    ...overrides,
  };
}

function bitmapItem(overrides: Partial<PinBoardItem> = {}): PinBoardItem {
  return {
    boardId: 1,
    itemId: 9,
    kind: "bitmap",
    width: 100,
    height: 100,
    order: 9,
    layer: 1,
    deleted: false,
    points: [[0, 0], [100, 0], [100, 100], [0, 100]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
    ...overrides,
  } as PinBoardItem;
}

describe("pin-board board snapshot", () => {
  it("clones geometry and carries a text payload only for text items", () => {
    const text = textItem();
    const bitmap = bitmapItem();
    const byId = new Map<number, PinBoardItem>([[text.itemId, text], [bitmap.itemId, bitmap]]);

    const snapshots = snapshotImages(byId, [text.itemId, bitmap.itemId, 99]);

    expect(snapshots).toHaveLength(2);
    expect(snapshots[0].points).not.toBe(text.points);
    expect(snapshots[0].points[0]).not.toBe(text.points[0]);
    expect(snapshots[0].text).toMatchObject({ text: "ab", width: 40, height: 26 });
    expect(snapshots[1].text).toBeUndefined();
  });

  it("restores content and reports only text signatures that actually changed", () => {
    const changed = textItem();
    const unchanged = textItem({ itemId: 8, text: "same" });
    const bitmap = bitmapItem();
    const byId = new Map<number, PinBoardItem>([
      [changed.itemId, changed],
      [unchanged.itemId, unchanged],
      [bitmap.itemId, bitmap],
    ]);

    const changedIds = restoreSnapshots(byId, [
      {
        itemId: changed.itemId,
        deleted: true,
        layer: 2,
        points: [[0, 0], [10, 0], [10, 10], [0, 10]],
        uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
        text: { text: "undo", fontSize: 20, color: "#112233", align: "center", width: 40, height: 26 },
      },
      {
        itemId: unchanged.itemId,
        deleted: false,
        layer: 1,
        points: unchanged.points,
        uv: unchanged.uv,
        text: { text: "same", fontSize: 20, color: "#112233", align: "center", width: 40, height: 26 },
      },
      {
        itemId: bitmap.itemId,
        deleted: false,
        layer: 1,
        points: bitmap.points,
        uv: bitmap.uv,
      },
    ]);

    expect(changed.deleted).toBe(true);
    expect(changed.layer).toBe(2);
    expect(changed.text).toBe("undo");
    expect(changedIds).toEqual([changed.itemId]);
  });

  it("counts text-only changes in the board state key", () => {
    const image = textItem();
    const before = boardStateKey([image]);

    image.text = "changed";

    expect(boardStateKey([image])).not.toBe(before);
  });

  it("sends text payloads only when the text signature moved past the saved value", () => {
    const changed = textItem();
    const unchanged = textItem({ itemId: 8, text: "same" });
    const bitmap = bitmapItem();
    const saved = new Map<number, string>([
      [changed.itemId, pinBoardTextSignature(textItem({ text: "old" }))],
      [unchanged.itemId, pinBoardTextSignature(unchanged)],
    ]);

    const editable = editableBoardImages([changed, unchanged, bitmap], saved);

    expect(editable.find((item) => item.itemId === changed.itemId)?.text).toMatchObject({
      text: "ab",
      measuredWidth: 40,
      measuredHeight: 26,
    });
    expect(editable.find((item) => item.itemId === unchanged.itemId)?.text).toBeUndefined();
    expect(editable.find((item) => item.itemId === bitmap.itemId)?.text).toBeUndefined();
  });

  it("clones a board item without sharing its quads", () => {
    const image = bitmapItem();
    const copy = cloneBoardItem(image);

    copy.points[0][0] = 50;
    copy.uv[0][1] = 1;

    expect(image.points[0][0]).toBe(0);
    expect(image.uv[0][1]).toBe(0);
  });
});
