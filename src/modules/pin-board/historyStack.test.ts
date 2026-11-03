import { describe, expect, it } from "vitest";
import { HistoryStack } from "./historyStack";
import type { ImageSnapshot } from "./boardSnapshot";

function snapshot(deleted: boolean): ImageSnapshot {
  return {
    itemId: 1,
    deleted,
    layer: 1,
    points: [[0, 0], [1, 0], [1, 1], [0, 1]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
  };
}

describe("pin-board history stack", () => {
  it("ignores entries whose before and after snapshots are identical", () => {
    const stack = new HistoryStack();
    stack.push([snapshot(false)], [snapshot(false)]);

    expect(stack.length).toBe(0);
    expect(stack.canUndo).toBe(false);
  });

  it("moves the cursor for undo and redo and reports the entry", () => {
    const stack = new HistoryStack();
    stack.push([snapshot(false)], [snapshot(true)], "delete");

    expect(stack.canUndo).toBe(true);
    expect(stack.canRedo).toBe(false);
    expect(stack.undo()?.after[0].deleted).toBe(true);
    expect(stack.canUndo).toBe(false);
    expect(stack.undo()).toBeNull();
    expect(stack.canRedo).toBe(true);
    expect(stack.redo()?.after[0].deleted).toBe(true);
    expect(stack.canRedo).toBe(false);
    expect(stack.redo()).toBeNull();
  });

  it("drops the redo branch when a new entry is pushed after undo", () => {
    const stack = new HistoryStack();
    stack.push([snapshot(false)], [snapshot(true)], "first");
    stack.push([snapshot(true)], [snapshot(false)], "second");

    stack.undo();
    stack.push([snapshot(true)], [snapshot(false)], "third");

    expect(stack.length).toBe(2);
    expect(stack.items.map((entry) => entry.mergeKey)).toEqual(["first", "third"]);
    expect(stack.canRedo).toBe(false);
  });

  it("merges consecutive entries inside the window and starts a new one afterwards", () => {
    let now = 0;
    const stack = new HistoryStack(100, 350, () => now);
    stack.push([snapshot(false)], [snapshot(true)], "drag");
    now = 200;
    stack.push([snapshot(true)], [snapshot(false)], "drag");

    expect(stack.length).toBe(1);
    expect(stack.items[0].after[0].deleted).toBe(false);

    now = 600;
    stack.push([snapshot(false)], [snapshot(true)], "drag");
    expect(stack.length).toBe(2);
  });

  it("does not merge a different key or an entry that is no longer at the tip", () => {
    let now = 0;
    const stack = new HistoryStack(100, 350, () => now);
    stack.push([snapshot(false)], [snapshot(true)], "drag");
    stack.push([snapshot(true)], [snapshot(false)], "other");
    expect(stack.length).toBe(2);

    stack.undo();
    now = 100;
    stack.push([snapshot(false)], [snapshot(true)], "drag");
    expect(stack.length).toBe(2);
  });

  it("keeps only the newest entries when the limit is exceeded", () => {
    const stack = new HistoryStack(2);
    let value = false;
    for (let index = 0; index < 3; index += 1) {
      stack.push([snapshot(value)], [snapshot(!value)]);
      value = !value;
    }

    expect(stack.length).toBe(2);
    expect(stack.position).toBe(2);
  });
});
