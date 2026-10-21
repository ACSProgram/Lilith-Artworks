import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PinBoardItem } from "./types";

const api = vi.hoisted(() => ({
  savePinBoard: vi.fn(),
  finalizePinBoard: vi.fn(),
}));

vi.mock("./api", () => ({ pinBoardApi: api }));

import {
  resetPendingSettleForTests,
  retryPendingSettle,
  stageBoardSettle,
} from "./pendingSettle";
import { usePendingSettle } from "./usePendingSettle";

const DELAY_MS = 800;

function item(itemId: number): PinBoardItem {
  return {
    boardId: 4,
    itemId,
    kind: "bitmap",
    width: 100,
    height: 100,
    order: itemId,
    layer: 1,
    deleted: false,
    points: [[0, 0], [10, 0], [10, 10], [0, 10]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
  };
}

function saved(revision: string) {
  return { saved: true, revision, nowStep: 1 };
}

function permanentError() {
  return Object.assign(new Error("画板已被其他窗口或程序修改，请重新打开后再编辑"), {
    code: "operation_failed",
    retryable: false,
  });
}

beforeEach(() => {
  api.savePinBoard.mockReset();
  api.finalizePinBoard.mockReset();
  api.savePinBoard.mockResolvedValue(saved("r2"));
});

afterEach(() => {
  resetPendingSettleForTests();
  vi.useRealTimers();
  cleanup();
});

describe("usePendingSettle", () => {
  it("shows nothing while the queue is empty", () => {
    const { result } = renderHook(() => usePendingSettle(DELAY_MS));

    expect(result.current).toBeNull();
  });

  it("does not flash for a settle that finishes within the delay", async () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => usePendingSettle(DELAY_MS));

    act(() => {
      stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] });
    });
    // 写回在延迟窗口内完成：提示从未出现，完成后的空队列也不出现。
    await act(async () => {
      await vi.advanceTimersByTimeAsync(DELAY_MS / 2);
    });
    expect(result.current).toBeNull();
    expect(api.savePinBoard).toHaveBeenCalledOnce();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(DELAY_MS * 2);
    });
    expect(result.current).toBeNull();
  });

  it("shows the pending count once a settle outlives the delay and hides when it lands", async () => {
    vi.useFakeTimers();
    let finish!: (value: unknown) => void;
    api.savePinBoard.mockImplementation(
      () => new Promise((resolve) => { finish = resolve; }),
    );
    const { result } = renderHook(() => usePendingSettle(DELAY_MS));

    act(() => {
      stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] });
    });
    // 延迟窗口内（含边界前一毫秒）保持安静。
    await act(async () => {
      await vi.advanceTimersByTimeAsync(DELAY_MS - 1);
    });
    expect(result.current).toBeNull();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(result.current).toEqual({ count: 1, failure: null });

    // 保存落地：队列清空，提示随之消失。
    finish(saved("r2"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(result.current).toBeNull();
  });

  it("shows a failure immediately without waiting for the delay", async () => {
    vi.useFakeTimers();
    api.savePinBoard.mockRejectedValue(permanentError());
    const { result } = renderHook(() => usePendingSettle(DELAY_MS));

    act(() => {
      stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    // 失败需要用户处理：不等延迟窗口，立即出现并保持。
    expect(result.current?.failure?.boardId).toBe(4);
    expect(result.current?.failure?.message).toContain("其他窗口");
    expect(result.current?.count).toBe(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(DELAY_MS * 2);
    });
    expect(result.current).not.toBeNull();
  });

  it("hides again after a retry succeeds", async () => {
    vi.useFakeTimers();
    api.savePinBoard.mockRejectedValueOnce(permanentError());
    const { result } = renderHook(() => usePendingSettle(DELAY_MS));

    act(() => {
      stageBoardSettle({ boardId: 4, revision: "r1", items: [item(1)] });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(result.current?.failure).not.toBeNull();

    // 用户点重试：这次写成功，失败与计数一起清掉。
    act(() => {
      retryPendingSettle();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    expect(api.savePinBoard).toHaveBeenCalledTimes(2);
    expect(result.current).toBeNull();
  });
});
