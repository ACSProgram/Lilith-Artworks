import { describe, expect, it } from "vitest";
import { textureEvictionOrder } from "./textureEviction";
import type { LoadedTexture } from "./rendererGpu";

function loaded(byteSize: number, lastUsed: number): LoadedTexture {
  return { byteSize, lastUsed } as unknown as LoadedTexture;
}

function ids(entries: Array<[number, LoadedTexture]>): number[] {
  return entries.map(([itemId]) => itemId);
}

describe("pin-board texture eviction order", () => {
  it("never proposes the protected item or an export-protected item", () => {
    const entries: Array<[number, LoadedTexture]> = [
      [1, loaded(10, 1)],
      [2, loaded(10, 2)],
      [3, loaded(10, 3)],
    ];

    const order = textureEvictionOrder(entries, {
      protectedItemId: 1,
      visibleIds: new Set(),
      exportProtectedIds: new Set([2]),
      isPreview: false,
      hasPreview: () => false,
    });

    expect(ids(order)).toEqual([3]);
  });

  it("evicts hidden textures before visible ones", () => {
    const entries: Array<[number, LoadedTexture]> = [
      [1, loaded(10, 1)],
      [2, loaded(99, 99)],
      [3, loaded(10, 3)],
    ];

    const order = textureEvictionOrder(entries, {
      visibleIds: new Set([3]),
      exportProtectedIds: null,
      isPreview: false,
      hasPreview: () => false,
    });

    expect(ids(order)).toEqual([1, 2, 3]);
  });

  it("prefers a hidden texture that already has a preview fallback", () => {
    const entries: Array<[number, LoadedTexture]> = [
      [1, loaded(60, 10)],
      [2, loaded(60, 20)],
    ];

    const order = textureEvictionOrder(entries, {
      visibleIds: new Set(),
      exportProtectedIds: null,
      isPreview: false,
      hasPreview: (itemId) => itemId === 1,
    });

    expect(ids(order)).toEqual([1, 2]);
  });

  it("evicts cheaper small textures before expensive large ones, then falls back to LRU", () => {
    const entries: Array<[number, LoadedTexture]> = [
      [1, loaded(60, 10)],
      [2, loaded(30, 20)],
      [3, loaded(30, 5)],
    ];

    const order = textureEvictionOrder(entries, {
      visibleIds: new Set(),
      exportProtectedIds: null,
      isPreview: false,
      hasPreview: () => true,
    });

    expect(ids(order)).toEqual([3, 2, 1]);
  });

  it("treats every preview as covered and orders preview eviction by bytes and age", () => {
    const entries: Array<[number, LoadedTexture]> = [
      [1, loaded(20, 10)],
      [2, loaded(10, 20)],
    ];

    const order = textureEvictionOrder(entries, {
      visibleIds: new Set(),
      exportProtectedIds: null,
      isPreview: true,
      hasPreview: () => false,
    });

    expect(ids(order)).toEqual([2, 1]);
  });
});
