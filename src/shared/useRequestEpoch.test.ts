import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useRequestEpoch } from "./useRequestEpoch";

describe("useRequestEpoch", () => {
  it("invalidates in-flight reads when the epoch advances", () => {
    const { result } = renderHook(() => useRequestEpoch());
    let first = 0;
    act(() => {
      first = result.current.begin();
    });
    expect(result.current.isCurrent(first)).toBe(true);

    act(() => {
      result.current.invalidate();
    });
    expect(result.current.isCurrent(first)).toBe(false);

    let second = 0;
    act(() => {
      second = result.current.begin();
    });
    expect(result.current.isCurrent(second)).toBe(true);
    expect(result.current.isCurrent(first)).toBe(false);
  });

  it("keeps stable function identities across renders", () => {
    const { result, rerender } = renderHook(() => useRequestEpoch());
    const first = result.current;
    rerender();
    expect(result.current.begin).toBe(first.begin);
    expect(result.current.invalidate).toBe(first.invalidate);
    expect(result.current.isCurrent).toBe(first.isCurrent);
  });
});
