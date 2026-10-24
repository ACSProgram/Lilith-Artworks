import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PinBoardItem } from "./types";

const api = vi.hoisted(() => ({
  savePinBoard: vi.fn(),
  finalizePinBoard: vi.fn(),
}));

vi.mock("./api", () => ({ pinBoardApi: api }));

import {
  flushPendingSettle,
  pendingSettleState,
  recordBoardRevision,
  resetPendingSettleForTests,
  resolveBoardRevision,
  retryPendingSettle,
  stageBoardSettle,
  takeBoardSettle,
} from "./pendingSettle";

function item(itemId: number, deleted = false): PinBoardItem {
  return {
    boardId: 4,
    itemId,
    kind: "bitmap",
    width: 100,
    height: 100,
    order: itemId,
    layer: 1,
    deleted,
    points: [[0, 0], [10, 0], [10, 10], [0, 10]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
  };
}

function busyError() {
  return Object.assign(new Error("仓库正在写入，请稍后重试"), {
    code: "repository_busy",
    retryable: true,
  });
}

function saved(revision: string) {
  return { saved: true, revision, nowStep: 1 };
}

/** 让排队的微任务跑完（写回是「发起 + await」的形状，一步就够）。 */
async function settleMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  api.savePinBoard.mockReset();
  api.finalizePinBoard.mockReset();
  api.savePinBoard.mockResolvedValue(saved("r2"));
  api.finalizePinBoard.mockResolvedValue(saved("r3"));
});

afterEach(() => {
  resetPendingSettleForTests();
  vi.useRealTimers();
});

describe("pin-board pending settle", () => {
  it("writes the staged snapshot with its base revision and drains the queue", async () => {
    stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1), item(2)] });

    await settleMicrotasks();

    expect(api.savePinBoard).toHaveBeenCalledOnce();
    const [boardId, items, revision] = api.savePinBoard.mock.calls[0];
    expect(boardId).toBe(4);
    expect(revision).toBe("r1");
    // 位图只提交位置/层级/删除态与两个四边形；文字载荷由下一条用例覆盖。
    expect(items).toEqual([
      {
        itemId: 1,
        order: 1,
        layer: 1,
        deleted: false,
        points: [[0, 0], [10, 0], [10, 10], [0, 10]],
        uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
      },
      expect.objectContaining({ itemId: 2 }),
    ]);
    expect(pendingSettleState().count).toBe(0);
    expect(pendingSettleState().failure).toBeNull();
    expect(resolveBoardRevision(4, "stale")).toBe("r2");
  });

  it("carries text payloads into the background save", async () => {
    const text: PinBoardItem = {
      boardId: 4,
      itemId: 9,
      kind: "text",
      width: 80,
      height: 30,
      order: 9,
      layer: 1,
      deleted: false,
      points: [[0, 0], [80, 0], [80, 30], [0, 30]],
      uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
      available: true,
      text: "切画板前改的字",
      fontFamily: "sans-serif",
      fontSize: 20,
      color: "#00ff00",
      align: "center",
    };

    stageBoardSettle({ boardId: 4, revision: "r1", items: [text] });
    await settleMicrotasks();

    const items = api.savePinBoard.mock.calls[0][1];
    expect(items).toEqual([
      expect.objectContaining({
        itemId: 9,
        text: {
          text: "切画板前改的字",
          fontSize: 20,
          color: "#00ff00",
          align: "center",
          measuredWidth: 80,
          measuredHeight: 30,
        },
      }),
    ]);
  });

  it("retries a busy write silently instead of reporting it", async () => {
    vi.useFakeTimers();
    api.savePinBoard.mockRejectedValueOnce(busyError()).mockResolvedValueOnce(saved("r2"));

    stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] });
    await vi.advanceTimersByTimeAsync(0);
    expect(api.savePinBoard).toHaveBeenCalledOnce();
    // 重试期间没有任何用户可见的失败。
    expect(pendingSettleState().failure).toBeNull();

    await vi.advanceTimersByTimeAsync(600);
    expect(api.savePinBoard).toHaveBeenCalledTimes(2);
    expect(pendingSettleState().count).toBe(0);
  });

  it("keeps a permanent failure for the user and stops retrying", async () => {
    vi.useFakeTimers();
    api.savePinBoard.mockRejectedValue(Object.assign(
      new Error("画板已被其他窗口或程序修改，请重新打开后再编辑"),
      { code: "operation_failed", retryable: false },
    ));

    stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] });
    await vi.advanceTimersByTimeAsync(0);
    expect(pendingSettleState().failure?.boardId).toBe(4);
    expect(pendingSettleState().failure?.message).toContain("其他窗口");
    // 快照留在队列里等用户处理，不会被静默丢弃。
    expect(pendingSettleState().count).toBe(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(api.savePinBoard).toHaveBeenCalledOnce();

    api.savePinBoard.mockResolvedValue(saved("r2"));
    retryPendingSettle();
    await vi.advanceTimersByTimeAsync(0);
    expect(api.savePinBoard).toHaveBeenCalledTimes(2);
    expect(pendingSettleState().count).toBe(0);
  });

  it("waits for the save that was already in flight before writing", async () => {
    let finishSave!: (value: boolean) => void;
    const inFlight = new Promise<boolean>((resolve) => {
      finishSave = resolve;
    });

    stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] }, inFlight);
    await settleMicrotasks();
    // 前一次保存还没落地：这时写回会拿到旧 revision，被后端判成冲突。
    expect(api.savePinBoard).not.toHaveBeenCalled();

    // 它落地时会把新 revision 记进账本，队列据此写回。
    recordBoardRevision(4, "r2");
    finishSave(true);
    await settleMicrotasks();
    expect(api.savePinBoard).toHaveBeenCalledOnce();
    expect(api.savePinBoard.mock.calls[0][2]).toBe("r2");
  });

  it("hands the content back to the renderer instead of writing it again", async () => {
    let finishSave!: (value: boolean) => void;
    const inFlight = new Promise<boolean>((resolve) => {
      finishSave = resolve;
    });
    stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] }, inFlight);
    await settleMicrotasks();

    const taken = takeBoardSettle(4);

    expect(taken?.items).toHaveLength(1);
    // 队列不再负责这块画板：内容回到渲染器手里，随它的下一次保存落库。
    expect(pendingSettleState().count).toBe(0);
    finishSave(true);
    await settleMicrotasks();
    expect(api.savePinBoard).not.toHaveBeenCalled();
  });

  it("prunes soft-deleted rows after the save lands", async () => {
    stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1), item(2, true)] });

    await settleMicrotasks();

    expect(api.finalizePinBoard).toHaveBeenCalledOnce();
    expect(api.finalizePinBoard).toHaveBeenCalledWith(4, "r2");
  });

  it("reports a flush that cannot drain inside its budget", async () => {
    api.savePinBoard.mockRejectedValue(busyError());
    stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] });

    await expect(flushPendingSettle(20)).resolves.toBe(false);
    expect(pendingSettleState().count).toBe(1);
  });

  it("flushes an empty queue without touching the repository", async () => {
    await expect(flushPendingSettle(20)).resolves.toBe(true);
    expect(api.savePinBoard).not.toHaveBeenCalled();
  });
});
