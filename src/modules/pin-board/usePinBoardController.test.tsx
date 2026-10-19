import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { APP_EVENT_VERSION, type AppEvent } from "../../shared/appEvents";
import {
  dispatchRuntimeEvent,
  resetRuntimeEventBridgeForTests,
} from "../../shared/useRuntimeSubscription";
import type { PinBoardRenderer } from "./renderer";
import type { PinBoardSummary, PinBoardView } from "./types";

const api = vi.hoisted(() => ({
  listPinBoards: vi.fn(),
  listPinBoardTrash: vi.fn(),
  loadPinBoard: vi.fn(),
  importPinBoardImages: vi.fn(),
  readPinBoardClipboardPaths: vi.fn(),
}));

const lifecycle = vi.hoisted(() => ({
  registerPinBoardLifecycleParticipant: vi.fn(),
}));

const dialog = vi.hoisted(() => ({
  open: vi.fn(),
}));

vi.mock("./api", () => ({
  pinBoardApi: api,
  pickPinBoardImagePaths: vi.fn(async () => ["C:\\images\\a.png"]),
  pickPinBoardExportDirectory: vi.fn(),
}));
vi.mock("./lifecycle", () => lifecycle);
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

import { usePinBoardController } from "./usePinBoardController";

const settings = {
  arrangementGapPx: 10,
  textureCacheLevel: "medium" as const,
  autosave: false,
  lockShortcut: "CommandOrControl+R",
  fullscreenShortcut: "F11",
};

const board: PinBoardSummary = {
  boardId: 1,
  artworkId: "artwork-1",
  artworkTitle: "Artwork",
  name: "Board",
  sortOrder: 0,
  deleted: false,
  deletedAt: null,
  imageCount: 0,
  createdMs: 0,
  updatedMs: 0,
};

const view: PinBoardView = {
  boardId: 1,
  name: "Board",
  items: [],
  missingTextures: 0,
  revision: "r1",
  nowStep: 0,
};

function fakeRenderer(): PinBoardRenderer {
  return {
    interactionState: {
      imageCount: 0,
      selectedCount: 0,
      canUndo: false,
      canRedo: false,
      dirty: false,
      saving: false,
      locked: false,
      zoomPercent: 100,
    },
    save: vi.fn(async () => true),
    placementPoint: [0, 0] as [number, number],
    placementGap: () => 10,
    applyMutationResult: vi.fn(),
    setActive: vi.fn(),
    setArrangementGapCssPixels: vi.fn(),
    setAutosaveEnabled: vi.fn(),
    setTextureCacheBudgets: vi.fn(),
    finishActiveInteraction: vi.fn(),
    finalize: vi.fn(async () => true),
  } as unknown as PinBoardRenderer;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  api.listPinBoards.mockResolvedValue([board]);
  api.listPinBoardTrash.mockResolvedValue([]);
  api.loadPinBoard.mockResolvedValue(view);
  api.readPinBoardClipboardPaths.mockResolvedValue([]);
  lifecycle.registerPinBoardLifecycleParticipant.mockReturnValue(vi.fn());
});

afterEach(() => {
  cleanup();
  resetRuntimeEventBridgeForTests();
  vi.clearAllMocks();
});

describe("usePinBoardController", () => {
  it("loads boards and registers the renderer lifecycle participant", async () => {
    const { result } = renderHook(() => usePinBoardController({
      artworkId: "artwork-1",
      active: true,
      settings,
    }));

    await waitFor(() => expect(result.current.view?.boardId).toBe(1));
    expect(result.current.selected).toBe(1);

    const renderer = fakeRenderer();
    const unregister = vi.fn();
    lifecycle.registerPinBoardLifecycleParticipant.mockReturnValue(unregister);

    act(() => result.current.setRenderer(renderer));
    expect(lifecycle.registerPinBoardLifecycleParticipant).toHaveBeenCalledWith(renderer);

    act(() => result.current.setRenderer(null));
    expect(unregister).toHaveBeenCalledTimes(1);
  });

  it("applies operation progress events to the current transfer", async () => {
    const renderer = fakeRenderer();
    const imported = deferred<{ view: PinBoardView; itemIds: number[] }>();
    api.importPinBoardImages.mockReturnValue(imported.promise);

    const { result } = renderHook(() => usePinBoardController({
      artworkId: "artwork-1",
      active: true,
      settings,
    }));
    await waitFor(() => expect(result.current.view?.boardId).toBe(1));
    act(() => result.current.setRenderer(renderer));

    let pending!: Promise<void>;
    act(() => {
      pending = result.current.importImages();
    });
    await waitFor(() => expect(api.importPinBoardImages).toHaveBeenCalled());

    act(() => {
      dispatchRuntimeEvent({
        version: APP_EVENT_VERSION,
        type: "operationProgress",
        domain: "pinBoard",
        operationId: "operation-1",
        phase: "正在导入图片",
        current: 1,
        total: 2,
        cancellable: true,
        completionRevision: 0,
        timestampMs: 1,
      } as AppEvent);
    });
    expect(result.current.transferProgress).toEqual({
      label: "正在导入图片",
      current: 1,
      total: 2,
    });

    await act(async () => {
      imported.resolve({ view, itemIds: [10] });
      await pending;
    });
    expect(result.current.transferProgress).toBeNull();
    expect(result.current.status).toBe("已导入 1 张图片");
  });
});
