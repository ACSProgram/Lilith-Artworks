import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useAsyncOperation } from "./useAsyncOperation";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe("useAsyncOperation", () => {
  it("tracks busy across concurrent operations", async () => {
    const { result } = renderHook(() => useAsyncOperation());
    const first = deferred<void>();
    const second = deferred<void>();

    let finished = 0;
    act(() => {
      void result.current.run(() => first.promise).then(() => { finished += 1; });
      void result.current.run(() => second.promise).then(() => { finished += 1; });
    });
    await waitFor(() => expect(result.current.busy).toBe(true));

    await act(async () => {
      first.resolve();
      await first.promise;
    });
    expect(result.current.busy).toBe(true);
    expect(finished).toBe(1);

    await act(async () => {
      second.resolve();
      await second.promise;
    });
    await waitFor(() => expect(result.current.busy).toBe(false));
    expect(finished).toBe(2);
  });

  it("reports the error and still rejects to the caller", async () => {
    const { result } = renderHook(() => useAsyncOperation());
    const onError = vi.fn();
    const failure = new Error("boom");

    await expect(act(async () => {
      await result.current.run(
        () => Promise.reject(failure),
        { onError },
      );
    })).rejects.toBe(failure);

    expect(onError).toHaveBeenCalledWith(failure);
    expect(result.current.busy).toBe(false);
  });
});
