import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const pending = vi.hoisted(() => ({
  flushPendingSettle: vi.fn(),
}));

vi.mock("./pendingSettle", () => pending);

import {
  preparePinBoardRuntimeChange,
  registerPinBoardLifecycleParticipant,
} from "./lifecycle";

let unregister: (() => void) | null = null;

function mountParticipant(finalize: () => Promise<boolean>) {
  const participant = {
    finishActiveInteraction: vi.fn(),
    finalize: vi.fn(finalize),
  };
  unregister?.();
  unregister = registerPinBoardLifecycleParticipant(participant);
  return participant;
}

describe("pin-board lifecycle", () => {
  beforeEach(() => {
    pending.flushPendingSettle.mockReset();
    pending.flushPendingSettle.mockResolvedValue(true);
  });

  afterEach(() => {
    unregister?.();
    unregister = null;
  });

  it("settles the open board and then drains the background queue", async () => {
    const participant = mountParticipant(async () => true);

    await preparePinBoardRuntimeChange();

    expect(participant.finishActiveInteraction).toHaveBeenCalledOnce();
    expect(participant.finalize).toHaveBeenCalledOnce();
    expect(pending.flushPendingSettle).toHaveBeenCalledOnce();
  });

  it("still drains the queue when no renderer is mounted", async () => {
    await preparePinBoardRuntimeChange();

    expect(pending.flushPendingSettle).toHaveBeenCalledOnce();
  });

  it("refuses the runtime change when the open board cannot be settled", async () => {
    mountParticipant(async () => false);

    await expect(preparePinBoardRuntimeChange()).rejects.toThrow("未能");
    // 当前画板都没写成功，队列里的快照也不再尝试：调用方会中止。
    expect(pending.flushPendingSettle).not.toHaveBeenCalled();
  });

  it("refuses the runtime change when the queue cannot be drained", async () => {
    mountParticipant(async () => true);
    pending.flushPendingSettle.mockResolvedValue(false);

    await expect(preparePinBoardRuntimeChange()).rejects.toThrow("后台结算队列未清空");
  });
});
