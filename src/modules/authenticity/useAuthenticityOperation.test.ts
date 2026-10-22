import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { deferred } from "./testSupport";
import { useAuthenticityOperation } from "./useAuthenticityOperation";

function options() {
  return {
    onError: vi.fn(),
    cancelOperation: vi.fn(async () => true),
    onCancelRejected: vi.fn(),
  };
}

describe("useAuthenticityOperation", () => {
  afterEach(() => cleanup());

  it("reports a cancelled preview once the task finishes after the cancel request", async () => {
    const opts = options();
    const { result } = renderHook(() => useAuthenticityOperation(opts));
    act(() => result.current.begin("preview"));

    let cancelTask!: Promise<void>;
    act(() => { cancelTask = result.current.cancel(); });
    expect(result.current.cancelling).toBe(true);
    await act(async () => { await cancelTask; });
    expect(opts.onError).not.toHaveBeenCalled();

    act(() => result.current.finish("preview", true));
    expect(opts.onError).toHaveBeenCalledWith("质量预览已取消。");
    expect(result.current.cancelling).toBe(false);
    expect(result.current.isActive()).toBe(false);
  });

  it("reports a cancelled publication once the task finishes after the cancel request", async () => {
    const opts = options();
    const { result } = renderHook(() => useAuthenticityOperation(opts));
    act(() => result.current.begin("publish"));

    await act(async () => { await result.current.cancel(); });
    act(() => result.current.finish("publish", true));
    expect(opts.onError).toHaveBeenCalledWith("签名发布已取消。");
  });

  it("reports the cancellation when the task ends before the cancel request is accepted", async () => {
    const opts = options();
    const pending = deferred<boolean>();
    opts.cancelOperation.mockReturnValue(pending.promise);
    const { result } = renderHook(() => useAuthenticityOperation(opts));
    act(() => result.current.begin("publish"));

    let cancelTask!: Promise<void>;
    act(() => { cancelTask = result.current.cancel(); });
    act(() => result.current.finish("publish", true));
    expect(opts.onError).not.toHaveBeenCalled();

    await act(async () => {
      pending.resolve(true);
      await cancelTask;
    });
    expect(opts.onError).toHaveBeenCalledWith("签名发布已取消。");
  });

  it("does not report a cancellation for a task that completed successfully", async () => {
    const opts = options();
    const { result } = renderHook(() => useAuthenticityOperation(opts));
    act(() => result.current.begin("preview"));

    await act(async () => { await result.current.cancel(); });
    act(() => result.current.finish("preview", false));
    expect(opts.onError).not.toHaveBeenCalled();
    expect(result.current.cancelling).toBe(false);
  });

  it("resets the wait indicator without dropping the active task", async () => {
    const opts = options();
    const { result } = renderHook(() => useAuthenticityOperation(opts));
    act(() => result.current.begin("preview"));
    await act(async () => { await result.current.cancel(); });
    expect(result.current.cancelling).toBe(true);

    act(() => result.current.resetCancelling());
    expect(result.current.cancelling).toBe(false);
    expect(result.current.isActive()).toBe(true);
  });

  it("resets controller busy state when the cancel request is not accepted", async () => {
    const opts = options();
    opts.cancelOperation.mockResolvedValue(false);
    const { result } = renderHook(() => useAuthenticityOperation(opts));
    act(() => result.current.begin("preview"));

    await act(async () => { await result.current.cancel(); });
    expect(opts.onCancelRejected).toHaveBeenCalledOnce();
    expect(result.current.cancelling).toBe(false);
    expect(result.current.isActive()).toBe(false);
    expect(result.current.isCancelRequested()).toBe(false);
  });

  it("reports a cancel command failure without leaving a pending notice", async () => {
    const opts = options();
    opts.cancelOperation.mockRejectedValue(new Error("取消失败"));
    const { result } = renderHook(() => useAuthenticityOperation(opts));
    act(() => result.current.begin("preview"));

    await act(async () => { await result.current.cancel(); });
    expect(opts.onError).toHaveBeenCalledWith("取消失败");
    expect(result.current.cancelling).toBe(false);
    expect(result.current.isCancelRequested()).toBe(false);

    act(() => result.current.finish("preview", false));
    expect(opts.onError).toHaveBeenCalledTimes(1);
  });
});
