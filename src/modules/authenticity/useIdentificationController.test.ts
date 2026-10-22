import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PreviewImage } from "./types";
import { decodeResult, deferred, previewImage, record } from "./testSupport";
import { useIdentificationController } from "./useIdentificationController";

const api = vi.hoisted(() => ({
  stageDroppedImage: vi.fn(),
  previewExternal: vi.fn(),
  previewTile: vi.fn(),
  decode: vi.fn(),
  searchRecords: vi.fn(),
}));
const dialog = vi.hoisted(() => ({ open: vi.fn() }));

vi.mock("./api", () => ({ authenticityApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

describe("useIdentificationController", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => cleanup());

  it("stages a dropped image before previewing it", async () => {
    api.stageDroppedImage.mockResolvedValue("C:/tmp/staged.png");
    api.previewExternal.mockResolvedValue(previewImage);
    const onError = vi.fn();
    const { result } = renderHook(() => useIdentificationController({ onError }));

    await act(async () => { await result.current.importDropped("dropped.png", "AAAA"); });

    expect(api.stageDroppedImage).toHaveBeenCalledWith({ fileName: "dropped.png", dataBase64: "AAAA" });
    expect(api.previewExternal).toHaveBeenCalledWith("C:/tmp/staged.png");
    expect(result.current.path).toBe("C:/tmp/staged.png");
    expect(result.current.preview).toEqual(previewImage);
    expect(result.current.busy).toBe(false);
    expect(onError).not.toHaveBeenCalledWith(expect.any(String));
  });

  it("reports staging failures without changing the current preview", async () => {
    api.stageDroppedImage.mockRejectedValue(new Error("拖入的图片数据无效"));
    const onError = vi.fn();
    const { result } = renderHook(() => useIdentificationController({ onError }));

    await act(async () => { await result.current.importDropped("dropped.png", "!!"); });

    expect(onError).toHaveBeenCalledWith("拖入的图片数据无效");
    expect(result.current.preview).toBeNull();
    expect(result.current.busy).toBe(false);
  });

  it("requests the loupe block from the current external image", async () => {
    api.stageDroppedImage.mockResolvedValue("C:/tmp/staged.png");
    api.previewExternal.mockResolvedValue(previewImage);
    api.previewTile.mockResolvedValue(previewImage);
    const { result } = renderHook(() => useIdentificationController({ onError: vi.fn() }));

    await act(async () => { await result.current.importDropped("dropped.png", "AAAA"); });
    await act(async () => {
      await result.current.previewLoupe({ x: 0, y: 0, width: 72, height: 72, maxEdge: 72 });
    });

    expect(api.previewTile).toHaveBeenCalledWith({
      source: "external",
      cacheToken: null,
      branchId: null,
      path: "C:/tmp/staged.png",
      x: 0,
      y: 0,
      width: 72,
      height: 72,
      maxEdge: 72,
    });
  });

  it("discards an older image preview that resolves after a newer one", async () => {
    const first = deferred<PreviewImage>();
    const second = deferred<PreviewImage>();
    dialog.open.mockResolvedValueOnce("C:/a.png").mockResolvedValueOnce("C:/b.png");
    api.previewExternal.mockImplementation((path: string) => path === "C:/a.png" ? first.promise : second.promise);
    const { result } = renderHook(() => useIdentificationController({ onError: vi.fn() }));

    let firstTask!: Promise<void>;
    act(() => { firstTask = result.current.choose(); });
    await waitFor(() => expect(api.previewExternal).toHaveBeenCalledTimes(1));
    let secondTask!: Promise<void>;
    act(() => { secondTask = result.current.choose(); });
    await waitFor(() => expect(api.previewExternal).toHaveBeenCalledTimes(2));

    await act(async () => {
      second.resolve(previewImage);
      await secondTask;
    });
    expect(result.current.path).toBe("C:/b.png");

    await act(async () => {
      first.resolve({ ...previewImage, width: 10 });
      await firstTask;
    });
    expect(result.current.path).toBe("C:/b.png");
    expect(result.current.preview).toEqual(previewImage);
  });

  it("keeps the newest decode result when responses arrive out of order", async () => {
    const first = deferred<ReturnType<typeof decodeResult>>();
    const second = deferred<ReturnType<typeof decodeResult>>();
    api.stageDroppedImage
      .mockResolvedValueOnce("C:/a.png")
      .mockResolvedValueOnce("C:/b.png");
    api.previewExternal.mockResolvedValue(previewImage);
    api.decode.mockImplementation((inputPath: string) => inputPath === "C:/a.png" ? first.promise : second.promise);
    const { result } = renderHook(() => useIdentificationController({ onError: vi.fn() }));

    await act(async () => { await result.current.importDropped("a.png", "AAAA"); });
    let firstTask!: Promise<void>;
    act(() => { firstTask = result.current.decode(); });
    await waitFor(() => expect(api.decode).toHaveBeenCalledWith("C:/a.png", null));

    await act(async () => { await result.current.importDropped("b.png", "BBBB"); });
    let secondTask!: Promise<void>;
    act(() => { secondTask = result.current.decode(); });
    await waitFor(() => expect(api.decode).toHaveBeenCalledWith("C:/b.png", null));

    await act(async () => {
      second.resolve(decodeResult("second"));
      await secondTask;
    });
    expect(result.current.result?.title).toBe("second");

    await act(async () => {
      first.resolve(decodeResult("first"));
      await firstTask;
    });
    expect(result.current.result?.title).toBe("second");
  });

  it("keeps the newest search results when responses arrive out of order", async () => {
    const first = deferred<ReturnType<typeof record>[]>();
    const second = deferred<ReturnType<typeof record>[]>();
    api.searchRecords.mockImplementation((query: string) => query === "old" ? first.promise : second.promise);
    const { result } = renderHook(() => useIdentificationController({ onError: vi.fn() }));

    act(() => result.current.setQuery("old"));
    let firstTask!: Promise<void>;
    act(() => { firstTask = result.current.searchRecords(); });
    await waitFor(() => expect(api.searchRecords).toHaveBeenCalledWith("old"));

    act(() => result.current.setQuery("new"));
    let secondTask!: Promise<void>;
    act(() => { secondTask = result.current.searchRecords(); });
    await waitFor(() => expect(api.searchRecords).toHaveBeenCalledWith("new"));

    await act(async () => {
      second.resolve([record("second")]);
      await secondTask;
    });
    expect(result.current.records.map((item) => item.branchId)).toEqual(["second"]);

    await act(async () => {
      first.resolve([record("first")]);
      await firstTask;
    });
    expect(result.current.records.map((item) => item.branchId)).toEqual(["second"]);
  });
});
