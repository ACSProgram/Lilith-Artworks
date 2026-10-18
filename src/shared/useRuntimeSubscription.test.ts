import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { APP_EVENT_VERSION, type AppEvent } from "./appEvents";
import {
  dispatchRuntimeEvent,
  resetRuntimeEventBridgeForTests,
  setRuntimeEventRepositoryKey,
  useRuntimeSubscription,
} from "./useRuntimeSubscription";

function event(overrides: Partial<AppEvent> = {}): AppEvent {
  return {
    version: APP_EVENT_VERSION,
    type: "operationFinished",
    cancellable: true,
    completionRevision: 1,
    timestampMs: 1,
    outcome: "succeeded",
    ...overrides,
  } as AppEvent;
}

afterEach(() => {
  resetRuntimeEventBridgeForTests();
});

describe("useRuntimeSubscription", () => {
  it("fans one event out to every subscriber", () => {
    const first = vi.fn();
    const second = vi.fn();
    renderHook(() => useRuntimeSubscription(first));
    renderHook(() => useRuntimeSubscription(second));

    act(() => {
      dispatchRuntimeEvent(event({ timestampMs: 10 }));
    });

    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(1);
  });

  it("drops events from another repository", () => {
    const listener = vi.fn();
    renderHook(() => useRuntimeSubscription(listener, { repositoryKey: "C:\\Repo" }));

    act(() => {
      dispatchRuntimeEvent(event({ repositoryKey: "C:/other", timestampMs: 10 }));
    });
    expect(listener).not.toHaveBeenCalled();

    act(() => {
      dispatchRuntimeEvent(event({ repositoryKey: "\\\\?\\c:\\repo", timestampMs: 11 }));
    });
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("drops out-of-order events and unsupported versions", () => {
    const listener = vi.fn();
    renderHook(() => useRuntimeSubscription(listener));

    act(() => {
      dispatchRuntimeEvent(event({ timestampMs: 20 }));
      dispatchRuntimeEvent(event({ timestampMs: 10 }));
      dispatchRuntimeEvent(event({ timestampMs: 30, version: APP_EVENT_VERSION + 1 }));
      dispatchRuntimeEvent(event({ timestampMs: 21 }));
    });

    expect(listener).toHaveBeenCalledTimes(2);
  });

  it("ignores a trailing separator when comparing repository keys", () => {
    const listener = vi.fn();
    renderHook(() => useRuntimeSubscription(listener, { repositoryKey: "C:\\Repo\\" }));

    act(() => {
      dispatchRuntimeEvent(event({ repositoryKey: "c:/repo", timestampMs: 10 }));
    });

    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("restarts the timestamp window when the repository changes", () => {
    const listener = vi.fn();
    renderHook(() => useRuntimeSubscription(listener));

    act(() => {
      setRuntimeEventRepositoryKey("C:\\First");
      dispatchRuntimeEvent(event({ repositoryKey: "C:\\First", timestampMs: 900 }));
      setRuntimeEventRepositoryKey("C:\\Second");
      // 新仓库的事件时间戳小于上一个仓库，不能当成过期事件丢弃。
      dispatchRuntimeEvent(event({ repositoryKey: "C:\\Second", timestampMs: 100 }));
    });

    expect(listener).toHaveBeenCalledTimes(2);
  });

  it("honors the repository scope set by the application shell", () => {
    const listener = vi.fn();
    renderHook(() => useRuntimeSubscription(listener));
    setRuntimeEventRepositoryKey("C:\\Repo");

    act(() => {
      dispatchRuntimeEvent(event({ repositoryKey: "C:\\Other", timestampMs: 10 }));
      dispatchRuntimeEvent(event({ repositoryKey: "C:\\Repo", timestampMs: 11 }));
    });

    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("stops delivering after the subscriber unmounts", () => {
    const listener = vi.fn();
    const { unmount } = renderHook(() => useRuntimeSubscription(listener));
    unmount();

    act(() => {
      dispatchRuntimeEvent(event({ timestampMs: 10 }));
    });

    expect(listener).not.toHaveBeenCalled();
  });
});
