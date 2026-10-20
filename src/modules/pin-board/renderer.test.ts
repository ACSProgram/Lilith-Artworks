import { afterEach, describe, expect, it, vi } from "vitest";
import { viewportBounds, type Viewport } from "./geometry";
import { PinBoardRenderer } from "./renderer";
import { pinBoardApi } from "./api";
import { isAdditiveSelectionModifier, marqueeSelection, sameSelection } from "./selection";
import {
  pendingSettleState,
  resetPendingSettleForTests,
  resolveBoardRevision,
} from "./pendingSettle";
import type { PinBoardItem, PinBoardView } from "./types";
import { STROKE_TOOL, decodeStrokeHeader, type PinBoardStrokeGeometry } from "./strokeGeometry";
import { encodeStrokeGeometry } from "./strokeRender";
import { beginStrokeDraft, extendStrokeDraft } from "./strokeDrawing";
import {
  PIN_BOARD_MIN_PREVIEW_DIMENSION,
  PIN_BOARD_PREVIEW_TEXTURE_BUDGET_BYTES,
  PIN_BOARD_TEXTURE_RESIDENT_BUDGET_BYTES,
  textureBudgetsForLevel,
} from "./texturePolicy";

afterEach(() => {
  vi.unstubAllGlobals();
});

type TestLoadedTexture = {
  byteSize: number;
  lastUsed: number;
  texture: { destroy: ReturnType<typeof vi.fn> };
  vertexBuffer: { destroy: ReturnType<typeof vi.fn> };
  outlineBuffer: { destroy: ReturnType<typeof vi.fn> };
};

function makeLoaded(byteSize: number, lastUsed: number): TestLoadedTexture {
  const destroy = vi.fn();
  return {
    byteSize,
    lastUsed,
    texture: { destroy },
    vertexBuffer: { destroy },
    outlineBuffer: { destroy },
  };
}

function rendererWith(options: Record<string, unknown>): PinBoardRenderer {
  const renderer = Object.create(PinBoardRenderer.prototype) as PinBoardRenderer;
  Object.assign(renderer, {
    textures: new Map(),
    previewTextures: new Map(),
    failedTextureDimensions: new Map(),
    visibleImages: [],
    // 笔迹相关状态：真实实例由类字段初始化，替身对象要手工补齐。
    strokeGeometries: new Map(),
    strokeOutlineIndex: new Map(),
    strokeVersion: 0,
    strokeSignature: "",
    strokeLoadToken: 0,
    strokeIndexCount: 0,
    // 基准预算（低档）：与历史常量一致，预算相关的断言依赖这些值。
    textureBudgetBytes: PIN_BOARD_TEXTURE_RESIDENT_BUDGET_BYTES,
    previewTextureBudgetBytes: PIN_BOARD_PREVIEW_TEXTURE_BUDGET_BYTES,
    ...options,
  });
  return renderer;
}

function boardImage(itemId: number, points: PinBoardItem["points"], deleted = false): PinBoardItem {
  return {
    boardId: 1,
    itemId,
    kind: "bitmap",
    width: 100,
    height: 100,
    order: itemId,
    layer: 1,
    deleted,
    points,
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
  };
}

type FitTestRenderer = {
  applyContentFit: (cssWidth: number, cssHeight: number) => void;
  viewport: Viewport;
  pendingViewportFit: boolean;
};

describe("pin-board renderer keep-alive", () => {
  it("pauses texture work without releasing loaded textures", () => {
    const clearTimeout = vi.fn();
    vi.stubGlobal("window", { clearTimeout });
    const loadedTexture = { marker: "loaded" };
    const finishActiveInteraction = vi.fn();
    const closeContextMenu = vi.fn();
    const refreshViewport = vi.fn();
    const renderer = rendererWith({
      active: true,
      destroyed: false,
      generation: 7,
      loadRequested: true,
      loadTimer: 42,
      textures: new Map([[1, loadedTexture]]),
      finishActiveInteraction,
      onContextMenu: closeContextMenu,
      refreshViewport,
    });

    renderer.setActive(false);

    expect(finishActiveInteraction).toHaveBeenCalledOnce();
    expect(closeContextMenu).toHaveBeenCalledWith(null);
    expect(clearTimeout).toHaveBeenCalledWith(42);
    expect((renderer as unknown as { generation: number }).generation).toBe(8);
    expect((renderer as unknown as { loadRequested: boolean }).loadRequested).toBe(false);
    expect((renderer as unknown as { textures: Map<number, unknown> }).textures.get(1))
      .toBe(loadedTexture);

    renderer.setActive(true);
    expect(refreshViewport).toHaveBeenCalledWith(0);
    expect((renderer as unknown as { textures: Map<number, unknown> }).textures.get(1))
      .toBe(loadedTexture);
  });

  it("reserves space for an incoming high-zoom texture before loading it", () => {
    const renderer = rendererWith({
      textures: new Map([
        [1, makeLoaded(PIN_BOARD_TEXTURE_RESIDENT_BUDGET_BYTES - 8 * 1024 * 1024, 1)],
        [2, makeLoaded(8 * 1024 * 1024, 2)],
      ]),
    });

    (renderer as unknown as {
      evictTextures: (protectedImageId?: number, additionalBytes?: number) => void;
    }).evictTextures(1, 16 * 1024 * 1024);

    expect((renderer as unknown as { textures: Map<number, unknown> }).textures.has(2)).toBe(false);
    expect((renderer as unknown as { textures: Map<number, unknown> }).textures.has(1)).toBe(true);
  });

  it("counts in-flight reservations when admitting a second texture", () => {
    const renderer = rendererWith({
      textures: new Map([
        [1, makeLoaded(40 * 1024 * 1024, 1)],
        [2, makeLoaded(80 * 1024 * 1024, 2)],
      ]),
      textureReservedBytes: 0,
      draw: vi.fn(),
    });

    const reserve = (renderer as unknown as {
      reserveTextureBytes: (protectedImageId: number, bytes: number) => boolean;
    }).reserveTextureBytes(1, 16 * 1024 * 1024);

    expect(reserve).toBe(true);
    expect((renderer as unknown as { textures: Map<number, unknown> }).textures.has(1)).toBe(true);
    expect((renderer as unknown as { textures: Map<number, unknown> }).textures.has(2)).toBe(false);
    expect((renderer as unknown as { textureReservedBytes: number }).textureReservedBytes)
      .toBe(16 * 1024 * 1024);
  });

  it("keeps preview textures resident under the shared eviction budget", () => {
    const renderer = rendererWith({
      textures: new Map([
        [1, makeLoaded(PIN_BOARD_TEXTURE_RESIDENT_BUDGET_BYTES - 4 * 1024 * 1024, 1)],
      ]),
      previewTextures: new Map([
        [2, makeLoaded(4 * 1024 * 1024, 2)],
      ]),
    });

    // 高清预算超限时只淘汰 textures，常驻缩略图不受影响。
    (renderer as unknown as {
      evictTextures: (protectedImageId?: number, additionalBytes?: number) => void;
    }).evictTextures(1, 16 * 1024 * 1024);

    expect((renderer as unknown as { textures: Map<number, unknown> }).textures.has(1)).toBe(true);
    expect((renderer as unknown as { previewTextures: Map<number, unknown> }).previewTextures.has(2))
      .toBe(true);
  });

  it("evicts preview textures against their own budget", () => {
    const renderer = rendererWith({
      previewTextures: new Map([
        [1, makeLoaded(PIN_BOARD_PREVIEW_TEXTURE_BUDGET_BYTES - 4 * 1024 * 1024, 1)],
        [2, makeLoaded(4 * 1024 * 1024, 2)],
      ]),
    });

    (renderer as unknown as {
      evictTextures: (protectedImageId?: number, additionalBytes?: number, isPreview?: boolean) => void;
    }).evictTextures(1, 16 * 1024 * 1024, true);

    expect((renderer as unknown as { previewTextures: Map<number, unknown> }).previewTextures.has(2))
      .toBe(false);
    expect((renderer as unknown as { previewTextures: Map<number, unknown> }).previewTextures.has(1))
      .toBe(true);
  });

  it("prefers evicting high-zoom textures that still have a preview fallback", () => {
    const renderer = rendererWith({
      textures: new Map([
        // 两张不可见的高清纹理：id 1 有缩略图兜底，id 2 没有。
        [1, makeLoaded(60 * 1024 * 1024, 10)],
        [2, makeLoaded(60 * 1024 * 1024, 20)],
      ]),
      previewTextures: new Map([
        [1, makeLoaded(256 * 1024, 10)],
      ]),
    });

    (renderer as unknown as {
      evictTextures: (protectedImageId?: number, additionalBytes?: number) => void;
    }).evictTextures(undefined, 16 * 1024 * 1024);

    const textures = (renderer as unknown as { textures: Map<number, unknown> }).textures;
    expect(textures.has(1)).toBe(false);
    expect(textures.has(2)).toBe(true);
  });

  it("keeps expensive large textures by evicting cheaper small ones first", () => {
    const renderer = rendererWith({
      textures: new Map([
        // 大图纹理（60 MiB，重新解码成本高）与小图纹理（30 MiB）都不可见且有缩略图兜底。
        [1, makeLoaded(60 * 1024 * 1024, 10)],
        [2, makeLoaded(30 * 1024 * 1024, 20)],
      ]),
      previewTextures: new Map([
        [1, makeLoaded(256 * 1024, 10)],
        [2, makeLoaded(256 * 1024, 20)],
      ]),
    });

    // 90 MiB + 40 MiB 超过 128 MiB 预算：应优先淘汰重生成便宜的小图（id 2），
    // 保留重生成昂贵的大图（id 1），避免划走再划回反复等待大图。
    (renderer as unknown as {
      evictTextures: (protectedImageId?: number, additionalBytes?: number) => void;
    }).evictTextures(undefined, 40 * 1024 * 1024);

    const textures = (renderer as unknown as { textures: Map<number, unknown> }).textures;
    expect(textures.has(1)).toBe(true);
    expect(textures.has(2)).toBe(false);
  });

  it("generates preview candidates first so scrolling never blanks a board image", () => {
    const points = [[0, 0], [400, 0], [400, 300], [0, 300]] as unknown as PinBoardItem["points"];
    const image = {
      itemId: 7,
      kind: "bitmap",
      width: 4000,
      height: 3000,
      points,
    } as unknown as PinBoardItem;
    const renderer = rendererWith({
      visibleImages: [image],
      previewTextures: new Map(),
      textures: new Map(),
      textureQualityCaps: new Map(),
      selectedIds: new Set(),
      viewport: { centerX: 200, centerY: 150 },
      gpu: { device: { limits: { maxTextureDimension2D: 8192 } } },
      canvas: { height: 800 },
      requestedDimension: () => 2048,
    });

    const candidates = (renderer as unknown as {
      textureLoadCandidates: () => Array<{ isPreview?: boolean; maxDimension: number }>;
    }).textureLoadCandidates();

    const preview = candidates.filter((candidate) => candidate.isPreview);
    const detail = candidates.filter((candidate) => !candidate.isPreview);
    expect(preview).toHaveLength(1);
    expect(preview[0].maxDimension).toBe(PIN_BOARD_MIN_PREVIEW_DIMENSION);
    expect(detail).toHaveLength(1);
    expect(detail[0].maxDimension).toBe(2048);
    expect(candidates[0].isPreview).toBe(true);
  });

  it("skips the preview candidate once a thumbnail is already resident", () => {
    const points = [[0, 0], [400, 0], [400, 300], [0, 300]] as unknown as PinBoardItem["points"];
    const image = {
      itemId: 7,
      kind: "bitmap",
      width: 4000,
      height: 3000,
      points,
    } as unknown as PinBoardItem;
    const renderer = rendererWith({
      visibleImages: [image],
      previewTextures: new Map([[7, makeLoaded(256 * 1024, 1)]]),
      textures: new Map(),
      textureQualityCaps: new Map(),
      selectedIds: new Set(),
      viewport: { centerX: 200, centerY: 150 },
      gpu: { device: { limits: { maxTextureDimension2D: 8192 } } },
      canvas: { height: 800 },
      requestedDimension: () => 2048,
    });

    const candidates = (renderer as unknown as {
      textureLoadCandidates: () => Array<{ isPreview?: boolean; maxDimension: number }>;
    }).textureLoadCandidates();

    expect(candidates.some((candidate) => candidate.isPreview)).toBe(false);
    expect(candidates.some((candidate) => !candidate.isPreview)).toBe(true);
  });

  it("re-evicts under the new budget after a cache level change", () => {
    const renderer = rendererWith({
      textures: new Map([
        [1, makeLoaded(120 * 1024 * 1024, 1)],
        [2, makeLoaded(120 * 1024 * 1024, 2)],
      ]),
      textureBudgetBytes: 512 * 1024 * 1024,
      visibleImages: [],
    });

    // 切到低档（128 MiB 高清预算）：240 MiB 超限，按不可见 + 字节/时间淘汰一张。
    (renderer as unknown as {
      setTextureCacheBudgets: (budgets: { previewBytes: number; residentBytes: number }) => void;
    }).setTextureCacheBudgets(textureBudgetsForLevel("low"));

    const textures = (renderer as unknown as { textures: Map<number, unknown> }).textures;
    expect(textures.has(1)).toBe(false);
    expect(textures.has(2)).toBe(true);
  });

  it("fits an opened board to the minimal bounding box of its live images", () => {
    const renderer = rendererWith({
      images: [
        boardImage(1, [[0, 300], [400, 300], [400, 0], [0, 0]]),
        boardImage(2, [[1000, 300], [1400, 300], [1400, 0], [1000, 0]]),
        // 已删除的图片不参与包围框，否则会把视图拉向早已移出画板的位置。
        boardImage(3, [[9000, 9300], [9400, 9300], [9400, 9000], [9000, 9000]], true),
      ],
      viewport: { centerX: 0, centerY: 0, height: 5000 },
      defaultWorldUnitsPerCssPixel: 5,
      pendingViewportFit: true,
    });

    (renderer as unknown as FitTestRenderer).applyContentFit(1000, 600);

    const { viewport, pendingViewportFit } = renderer as unknown as FitTestRenderer;
    expect(pendingViewportFit).toBe(false);
    expect(viewport.centerX).toBe(700);
    expect(viewport.centerY).toBe(150);
    const view = viewportBounds(viewport, 1000, 600);
    expect(view.minX).toBeLessThanOrEqual(0);
    expect(view.maxX).toBeGreaterThanOrEqual(1400);
    expect(view.minY).toBeLessThanOrEqual(0);
    expect(view.maxY).toBeGreaterThanOrEqual(300);
  });

  it("falls back to the default view when a board has no image", () => {
    const renderer = rendererWith({
      images: [],
      viewport: { centerX: 900, centerY: 900, height: 100 },
      defaultWorldUnitsPerCssPixel: 5,
      pendingViewportFit: true,
    });

    (renderer as unknown as FitTestRenderer).applyContentFit(1000, 600);

    const { viewport } = renderer as unknown as FitTestRenderer;
    expect(viewport.centerX).toBe(0);
    expect(viewport.centerY).toBe(0);
    expect(viewport.height).toBe(3000);
  });
});

type AutosaveTestRenderer = {
  afterModelChange: (itemIds: number[]) => void;
  save: () => Promise<boolean>;
  destroy: (finalize?: boolean) => void;
  setAutosaveEnabled: (enabled: boolean) => void;
  autosaveTimer: number | null;
  autosaveEnabled: boolean;
  dirty: boolean;
  destroyed: boolean;
};

function autosaveRenderer(options: Record<string, unknown>): AutosaveTestRenderer {
  return rendererWith({
    destroyed: false,
    autosaveTimer: null,
    // 设置页开关默认关闭；需要验证排程行为时在用例里显式开启。
    autosaveEnabled: false,
    dirty: false,
    savedStateKey: "saved",
    selectedIds: new Set<number>(),
    imageById: new Map(),
    images: [],
    // 默认模拟“内存状态与已保存状态不同”，让 afterModelChange 判定为脏。
    stateKey: () => "current",
    syncGeometryBuffers: vi.fn(),
    refreshViewport: vi.fn(),
    emitState: vi.fn(),
    ...options,
  }) as unknown as AutosaveTestRenderer;
}

describe("pin-board stroke rendering", () => {
  const strokeQuad: PinBoardItem["points"] = [[0, 0], [100, 0], [100, 100], [0, 100]];

  function strokeItem(itemId = 5): PinBoardItem {
    return {
      boardId: 1,
      itemId,
      kind: "stroke",
      width: 100,
      height: 100,
      order: itemId,
      layer: 3,
      deleted: false,
      points: strokeQuad,
      uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
      available: true,
      tool: STROKE_TOOL.line,
      color: "#FF8000",
      strokeWidth: 2,
    } as unknown as PinBoardItem;
  }

  function strokeGeometry(): PinBoardStrokeGeometry {
    const bytes = encodeStrokeGeometry(STROKE_TOOL.line, [], 0);
    return {
      ...decodeStrokeHeader(bytes),
      itemId: 5,
      color: "#FF8000",
      strokeWidth: 2,
      width: 100,
      height: 100,
      geometry: bytes,
    };
  }

  it("never queues a texture load for a stroke item", () => {
    const renderer = rendererWith({
      visibleImages: [strokeItem()],
      textureQualityCaps: new Map(),
      selectedIds: new Set<number>(),
      viewport: { centerX: 50, centerY: 50, height: 100 },
      gpu: { device: { limits: { maxTextureDimension2D: 8192 } } },
      canvas: { height: 100 },
    });

    const candidates = (renderer as unknown as {
      textureLoadCandidates: () => unknown[];
    }).textureLoadCandidates();

    // 笔迹进纹理队列会去读 DDS 并失败，最终变成红色失败占位框。
    expect(candidates).toHaveLength(0);
  });

  it("does not paint a loading placeholder over a stroke", () => {
    const layer = document.createElement("div");
    const renderer = rendererWith({
      visibleImages: [strokeItem()],
      placeholderElement: layer,
      placeholderNodes: new Map(),
      loaded: true,
      destroyed: false,
      gpuUnavailable: false,
      viewport: { centerX: 50, centerY: 50, height: 100 },
      canvas: {
        getBoundingClientRect: () => ({ width: 100, height: 100, left: 0, top: 0 }),
      },
    });

    (renderer as unknown as { updatePlaceholderOverlay: () => void }).updatePlaceholderOverlay();

    expect(layer.childElementCount).toBe(0);
  });

  it("builds the stroke buffer once and only rebuilds when zoom changes", () => {
    const created: number[] = [];
    const device = {
      createBuffer: (descriptor: { size: number }) => {
        created.push(descriptor.size);
        return {
          getMappedRange: () => new ArrayBuffer(descriptor.size),
          unmap: () => undefined,
          destroy: () => undefined,
        };
      },
    };
    const renderer = rendererWith({
      visibleImages: [strokeItem()],
      strokeGeometries: new Map([[5, strokeGeometry()]]),
      strokeVersion: 0,
      strokeSignature: "",
      strokeOutlineIndex: new Map(),
      strokeIndexCount: 0,
      gpu: { device },
      viewport: { centerX: 50, centerY: 50, height: 100 },
      canvas: { height: 100 },
      loaded: true,
      destroyed: false,
      boardId: 1,
      onError: vi.fn(),
    });
    const syncStrokeBuffers = () => (renderer as unknown as {
      syncStrokeBuffers: () => void;
    }).syncStrokeBuffers();

    syncStrokeBuffers();

    // 顶点 + 索引 + 选中描边三个缓冲。
    expect(created).toHaveLength(3);
    expect((renderer as unknown as { strokeIndexCount: number }).strokeIndexCount).toBe(6);

    syncStrokeBuffers();
    // 可见集合与缩放都没变：不重建。
    expect(created).toHaveLength(3);

    (renderer as unknown as { viewport: { height: number } }).viewport.height = 50;
    syncStrokeBuffers();
    expect(created).toHaveLength(6);
  });
});

describe("pin-board renderer autosave", () => {
  it("does not schedule a save while the setting is disabled", () => {
    const setTimeout = vi.fn();
    vi.stubGlobal("window", { setTimeout, clearTimeout: vi.fn() });
    const renderer = autosaveRenderer({});

    renderer.afterModelChange([]);

    expect(setTimeout).not.toHaveBeenCalled();
    expect(renderer.autosaveTimer).toBeNull();
  });

  it("schedules immediately when enabled while dirty", () => {
    const setTimeout = vi.fn(() => 101);
    vi.stubGlobal("window", { setTimeout, clearTimeout: vi.fn() });
    const renderer = autosaveRenderer({ dirty: true });

    renderer.setAutosaveEnabled(true);

    expect(setTimeout).toHaveBeenCalledOnce();
    expect(setTimeout).toHaveBeenLastCalledWith(expect.any(Function), 1500);
    expect(renderer.autosaveTimer).toBe(101);
  });

  it("cancels the pending timer when disabled", () => {
    const clearTimeout = vi.fn();
    vi.stubGlobal("window", { setTimeout: vi.fn(), clearTimeout });
    const renderer = autosaveRenderer({ autosaveEnabled: true, autosaveTimer: 77 });

    renderer.setAutosaveEnabled(false);

    expect(clearTimeout).toHaveBeenCalledWith(77);
    expect(renderer.autosaveTimer).toBeNull();
  });

  it("schedules a debounced save after a model change", () => {
    const setTimeout = vi.fn(() => 101);
    vi.stubGlobal("window", { setTimeout, clearTimeout: vi.fn() });
    const renderer = autosaveRenderer({ autosaveEnabled: true });

    renderer.afterModelChange([]);

    expect(setTimeout).toHaveBeenCalledOnce();
    expect(setTimeout).toHaveBeenLastCalledWith(expect.any(Function), 1500);
    expect(renderer.autosaveTimer).toBe(101);
    expect(renderer.dirty).toBe(true);
  });

  it("reschedules when another change arrives before the timer fires", () => {
    const setTimeout = vi.fn().mockReturnValueOnce(101).mockReturnValueOnce(202);
    const clearTimeout = vi.fn();
    vi.stubGlobal("window", { setTimeout, clearTimeout });
    const renderer = autosaveRenderer({ autosaveEnabled: true });

    renderer.afterModelChange([]);
    renderer.afterModelChange([]);

    expect(setTimeout).toHaveBeenCalledTimes(2);
    expect(clearTimeout).toHaveBeenCalledWith(101);
    expect(renderer.autosaveTimer).toBe(202);
  });

  it("does not schedule a save when the state is already persisted", () => {
    const setTimeout = vi.fn();
    vi.stubGlobal("window", { setTimeout, clearTimeout: vi.fn() });
    const renderer = autosaveRenderer({
      autosaveEnabled: true,
      stateKey: () => "saved",
    });

    renderer.afterModelChange([]);

    expect(setTimeout).not.toHaveBeenCalled();
    expect(renderer.autosaveTimer).toBeNull();
  });

  it("cancels the pending autosave when an explicit save starts", () => {
    const clearTimeout = vi.fn();
    vi.stubGlobal("window", { setTimeout: vi.fn(), clearTimeout });
    const renderer = autosaveRenderer({
      autosaveTimer: 77,
      stateKey: () => "saved",
      savePromise: null,
      revision: "r1",
      boardId: 1,
      images: [],
      onError: vi.fn(),
    });

    void renderer.save();

    expect(clearTimeout).toHaveBeenCalledWith(77);
    expect(renderer.autosaveTimer).toBeNull();
  });

  it("cancels the pending autosave on destroy", () => {
    const clearTimeout = vi.fn();
    vi.stubGlobal("window", {
      setTimeout: vi.fn(),
      clearTimeout,
      removeEventListener: vi.fn(),
    });
    const renderer = autosaveRenderer({
      autosaveTimer: 77,
      finalized: true,
      generation: 0,
      loadTimer: null,
      viewport: { centerX: 0, centerY: 0, height: 100 },
      locked: true,
      selectedIds: new Set<number>(),
      canvas: { removeEventListener: vi.fn() },
      resizeObserver: { disconnect: vi.fn() },
      themeObserver: { disconnect: vi.fn() },
      onSessionChange: vi.fn(),
      gpu: { viewportBuffer: { destroy: vi.fn() } },
      textures: new Map(),
      previewTextures: new Map(),
      worldUnitsPerCssPixel: () => 1,
    });

    renderer.destroy();

    expect(clearTimeout).toHaveBeenCalledWith(77);
    expect(renderer.autosaveTimer).toBeNull();
    expect(renderer.destroyed).toBe(true);
  });
});

type FinalizeTestRenderer = {
  finalize: () => Promise<boolean>;
  revision: string;
  finalized: boolean;
  onError: ReturnType<typeof vi.fn>;
};

function finalizeRenderer(): FinalizeTestRenderer {
  return rendererWith({
    loaded: true,
    finalized: false,
    finalizePromise: null,
    boardId: 7,
    revision: "r1",
    // 保存已经在正常路径里测过；这里只关心结算的 busy 重试。
    save: () => Promise.resolve(true),
    onError: vi.fn(),
  }) as unknown as FinalizeTestRenderer;
}

describe("pin-board renderer finalize", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("retries the settlement once when the repository is busy", async () => {
    const busy = Object.assign(new Error("仓库正在写入，请稍后重试"), {
      code: "repository_busy",
    });
    const finalizePinBoard = vi.spyOn(pinBoardApi, "finalizePinBoard")
      .mockRejectedValueOnce(busy)
      // 结算路径只读 revision；saved/nowStep 仅为满足 SavePinBoardResult。
      .mockResolvedValueOnce({ saved: true, revision: "r2", nowStep: 0 });
    const renderer = finalizeRenderer();

    await expect(renderer.finalize()).resolves.toBe(true);

    expect(finalizePinBoard).toHaveBeenCalledTimes(2);
    expect(renderer.revision).toBe("r2");
    expect(renderer.finalized).toBe(true);
    expect(renderer.onError).not.toHaveBeenCalled();
  });

  it("reports a non-busy settlement failure without retrying", async () => {
    const finalizePinBoard = vi
      .spyOn(pinBoardApi, "finalizePinBoard")
      .mockRejectedValue(new Error("画板已被其他窗口修改"));
    const renderer = finalizeRenderer();

    await expect(renderer.finalize()).resolves.toBe(false);

    expect(finalizePinBoard).toHaveBeenCalledOnce();
    expect(renderer.finalized).toBe(false);
    expect(renderer.onError).toHaveBeenCalledOnce();
  });
});

type SelectionTestRenderer = {
  selectedIds: Set<number>;
  selectAll: () => void;
};
describe("pin-board renderer select-all", () => {
  const quad: PinBoardItem["points"] = [[0, 0], [10, 0], [10, 10], [0, 10]];

  it("selects every non-deleted image on the whole board", () => {
    const refreshSelectionVisuals = vi.fn();
    const renderer = rendererWith({
      locked: false,
      images: [boardImage(1, quad), boardImage(2, quad), boardImage(3, quad, true)],
      selectedIds: new Set<number>([2]),
      refreshSelectionVisuals,
    });

    (renderer as unknown as SelectionTestRenderer).selectAll();

    const selected = [...(renderer as unknown as SelectionTestRenderer).selectedIds];
    expect(selected.sort((left, right) => left - right)).toEqual([1, 2]);
    expect(refreshSelectionVisuals).toHaveBeenCalledOnce();
  });

  it("ignores select-all while the board is locked", () => {
    const refreshSelectionVisuals = vi.fn();
    const renderer = rendererWith({
      locked: true,
      images: [boardImage(1, quad)],
      selectedIds: new Set<number>(),
      refreshSelectionVisuals,
    });

    (renderer as unknown as SelectionTestRenderer).selectAll();

    expect([...(renderer as unknown as SelectionTestRenderer).selectedIds]).toEqual([]);
    expect(refreshSelectionVisuals).not.toHaveBeenCalled();
  });
});

describe("pin-board additive marquee selection", () => {
  const topLeft: PinBoardItem["points"] = [[-450, 450], [-350, 450], [-350, 350], [-450, 350]];
  const bottomRight: PinBoardItem["points"] = [[350, -350], [450, -350], [450, -450], [350, -450]];
  const topLeftBounds = { minX: -500, minY: 300, maxX: -300, maxY: 500 };
  const bottomRightBounds = { minX: 300, minY: -500, maxX: 500, maxY: -300 };

  it("treats Ctrl, Cmd and Shift as additive-selection modifiers", () => {
    expect(isAdditiveSelectionModifier({ ctrlKey: true, metaKey: false, shiftKey: false })).toBe(true);
    expect(isAdditiveSelectionModifier({ ctrlKey: false, metaKey: true, shiftKey: false })).toBe(true);
    expect(isAdditiveSelectionModifier({ ctrlKey: false, metaKey: false, shiftKey: true })).toBe(true);
    expect(isAdditiveSelectionModifier({ ctrlKey: false, metaKey: false, shiftKey: false })).toBe(false);
  });

  it("adds framed images on top of the base while a plain marquee replaces it", () => {
    const images = [boardImage(1, topLeft), boardImage(2, bottomRight)];

    const additive = marqueeSelection(images, topLeftBounds, [2]);
    expect([...additive].sort((left, right) => left - right)).toEqual([1, 2]);

    const replacing = marqueeSelection(images, topLeftBounds, []);
    expect([...replacing]).toEqual([1]);
  });

  it("accumulates across repeated marquees without dropping the earlier selection", () => {
    const images = [boardImage(1, topLeft), boardImage(2, bottomRight)];

    const first = marqueeSelection(images, topLeftBounds, []);
    expect([...first]).toEqual([1]);

    const second = marqueeSelection(images, bottomRightBounds, first);
    expect([...second].sort((left, right) => left - right)).toEqual([1, 2]);
  });

  it("skips deleted images and images outside the marquee", () => {
    const images = [boardImage(1, topLeft), boardImage(2, bottomRight), boardImage(3, topLeft, true)];

    expect([...marqueeSelection(images, topLeftBounds, [])]).toEqual([1]);
  });

  it("leaves the base set untouched", () => {
    const base = new Set<number>([2]);

    marqueeSelection([boardImage(1, topLeft)], topLeftBounds, base);

    expect([...base]).toEqual([2]);
  });

  it("detects whether two selections are identical regardless of order", () => {
    expect(sameSelection(new Set([1, 2]), new Set([2, 1]))).toBe(true);
    expect(sameSelection(new Set(), new Set())).toBe(true);
    expect(sameSelection(new Set([1, 2]), new Set([1]))).toBe(false);
    expect(sameSelection(new Set([1, 2]), new Set([1, 3]))).toBe(false);
  });
});

type BoardSwitchRenderer = {
  loadBoard: (view: PinBoardView, session: null, stagePrevious?: boolean) => boolean;
  currentBoardId: number;
  interactionState: { dirty: boolean; imageCount: number };
};

const SWITCH_QUAD: PinBoardItem["points"] = [[0, 0], [10, 0], [10, 10], [0, 10]];

function boardView(boardId: number, items: PinBoardItem[] = []): PinBoardView {
  return { boardId, name: `Board ${boardId}`, items, missingTextures: 0, revision: `r${boardId}`, nowStep: 0 };
}

/**
 * 切换作品的关键是「装载同步完成、写回交给队列」：这里把画板级资源释放与重绘替身掉，
 * 只观察快照的交接。
 */
function boardSwitchRenderer(images: PinBoardItem[] = [boardImage(1, SWITCH_QUAD)], dirty = true): BoardSwitchRenderer {
  return rendererWith({
    loaded: true,
    boardId: 1,
    revision: "r1",
    dirty,
    finalized: false,
    images,
    imageById: new Map(images.map((image) => [image.itemId, image])),
    selectedIds: new Set<number>(),
    viewport: { centerX: 0, centerY: 0, height: 1 },
    autosaveTimer: null,
    savePromise: null,
    onSessionChange: vi.fn(),
    onState: vi.fn(),
    persistSession: vi.fn(),
    releaseBoardResources: vi.fn(),
    resize: vi.fn(),
    draw: vi.fn(),
    canvas: {
      getBoundingClientRect: () => ({ width: 100, height: 100 }),
      classList: { remove: vi.fn() },
    },
  }) as unknown as BoardSwitchRenderer;
}

describe("pin-board renderer board switch", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    resetPendingSettleForTests();
  });

  it("switches without waiting for the repository write", () => {
    // 写回卡在仓库里（后台任务持锁时就是这样）：切换仍必须立刻完成。
    vi.spyOn(pinBoardApi, "savePinBoard").mockReturnValue(new Promise(() => undefined));
    const renderer = boardSwitchRenderer();

    expect(renderer.loadBoard(boardView(2), null)).toBe(true);

    // 画布已经切到画板 2，而画板 1 的改动还压在后台队列里等仓库。
    expect(renderer.currentBoardId).toBe(2);
    expect(pendingSettleState().count).toBe(1);
  });

  it("stages nothing when the board has no unsaved edit", () => {
    const renderer = boardSwitchRenderer([boardImage(1, SWITCH_QUAD)], false);

    renderer.loadBoard(boardView(2), null);

    expect(pendingSettleState().count).toBe(0);
  });

  it("drops the board state without staging when the old board is gone", () => {
    const renderer = boardSwitchRenderer();

    renderer.loadBoard(boardView(2), null, false);

    expect(pendingSettleState().count).toBe(0);
  });

  it("restores the staged content when the user switches back immediately", () => {
    vi.spyOn(pinBoardApi, "savePinBoard").mockResolvedValue({ saved: true, revision: "r2", nowStep: 1 });
    const renderer = boardSwitchRenderer([boardImage(1, SWITCH_QUAD), boardImage(2, SWITCH_QUAD)]);

    renderer.loadBoard(boardView(2), null);
    expect(pendingSettleState().count).toBe(1);

    renderer.loadBoard(boardView(1), null);

    // 快照回到渲染器手里：画布恢复成离开时的内容，改动仍算未保存。
    expect(pendingSettleState().count).toBe(0);
    expect(renderer.interactionState.imageCount).toBe(2);
    expect(renderer.interactionState.dirty).toBe(true);
  });
});

describe("pin-board renderer mutation results", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    resetPendingSettleForTests();
  });

  function mutationRenderer(tool: "select" | "pen") {
    const item = boardImage(9, [[0, 0], [10, 0], [10, -10], [0, -10]]);
    const view: PinBoardView = {
      boardId: 1,
      name: "Board",
      items: [item],
      missingTextures: 0,
      revision: "r2",
      nowStep: 1,
    };
    const renderer = rendererWith({
      loaded: true,
      boardId: 1,
      revision: "r1",
      tool,
      images: [],
      imageById: new Map(),
      selectedIds: new Set<number>(),
      pushHistory: vi.fn(),
      afterModelChange: vi.fn(),
      stateKey: () => "state",
    });
    return { renderer, view, item };
  }

  it("advances the shared revision ledger with the mutation result", () => {
    const { renderer, view, item } = mutationRenderer("pen");

    renderer.applyMutationResult(view, [item.itemId]);

    // 新建命令推进了后端 revision：账本必须跟上，否则下一次保存会拿旧值撞冲突。
    expect(renderer.boardRevision).toBe("r2");
    expect(resolveBoardRevision(1, "r1")).toBe("r2");
  });

  it("does not select the created item while a drawing tool is active", () => {
    const { renderer, view, item } = mutationRenderer("pen");

    renderer.applyMutationResult(view, [item.itemId]);

    // 工具模式的指针只走绘制分支，自动选中只会留下一个拖不动、还挡下一笔的包围框。
    expect([...(renderer as unknown as { selectedIds: Set<number> }).selectedIds]).toEqual([]);
  });

  it("selects the created item when the select tool is active", () => {
    const { renderer, view, item } = mutationRenderer("select");

    renderer.applyMutationResult(view, [item.itemId]);

    expect([...(renderer as unknown as { selectedIds: Set<number> }).selectedIds]).toEqual([9]);
  });
});

describe("pin-board arrangement", () => {
  const textItem = (itemId: number): PinBoardItem => ({
    boardId: 1,
    itemId,
    kind: "text",
    width: 10,
    height: 10,
    order: itemId,
    layer: 1,
    deleted: false,
    points: [[0, 0], [80, 0], [80, -20], [0, -20]],
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
    text: "备注",
    fontFamily: "sans-serif",
    fontSize: 14,
    color: "#ffffff",
    align: "left",
  });

  it("sends only bitmap items to the native command and applies the result", async () => {
    const bitmaps = [
      boardImage(1, [[0, 0], [100, 0], [100, -50], [0, -50]]),
      boardImage(2, [[200, 0], [260, 0], [260, -40], [200, -40]]),
    ];
    const commitMutation = vi.fn();
    const renderer = rendererWith({
      images: [...bitmaps, textItem(3)],
      selectedIds: new Set([1, 2, 3]),
      locked: false,
      boardId: 1,
      arrangementGapRatio: 0.06,
      finishActiveInteraction: vi.fn(),
      commitMutation,
    });
    const arrange = vi.spyOn(pinBoardApi, "arrangePinBoardImages").mockResolvedValue([
      { itemId: 1, points: [[0, 0], [100, 0], [100, -50], [0, -50]] },
      { itemId: 2, points: [[106, 0], [166, 0], [166, -40], [106, -40]] },
    ]);

    await renderer.arrangeSelected();

    expect(arrange).toHaveBeenCalledTimes(1);
    const [items, ratio] = arrange.mock.calls[0];
    // 文字条目被排除：它的 quad 被拉伸时 fontSize 不会跟着变，字会被拉变形。
    expect(items.map((item) => item.itemId)).toEqual([1, 2]);
    expect(items[0]).toEqual({ itemId: 1, width: 100, height: 50, centerX: 50, centerY: -25 });
    expect(ratio).toBe(0.06);
    // 结果交给 commitMutation，撤销栈与脏标记因此与其它几何操作一致。
    expect(commitMutation).toHaveBeenCalledTimes(1);
    arrange.mockRestore();
  });

  it("does nothing when fewer than two bitmaps are selected", async () => {
    const renderer = rendererWith({
      images: [boardImage(1, [[0, 0], [100, 0], [100, -50], [0, -50]]), textItem(2)],
      selectedIds: new Set([1, 2]),
      locked: false,
      boardId: 1,
      arrangementGapRatio: 0.06,
      finishActiveInteraction: vi.fn(),
      commitMutation: vi.fn(),
    });
    const arrange = vi.spyOn(pinBoardApi, "arrangePinBoardImages").mockResolvedValue([]);

    await renderer.arrangeSelected();

    expect(arrange).not.toHaveBeenCalled();
    arrange.mockRestore();
  });
});

describe("pin-board drawing interaction", () => {
  /** 绘制路径只碰这几样 DOM：画布类名、指针捕获与预览层清理。 */
  function fakeCanvas() {
    return {
      classList: { add: vi.fn(), remove: vi.fn() },
      hasPointerCapture: () => false,
      releasePointerCapture: vi.fn(),
      getBoundingClientRect: () => ({ width: 100, height: 100, left: 0, top: 0 }),
    };
  }

  function drawingRenderer(options: Record<string, unknown> = {}) {
    return rendererWith({
      loaded: true,
      destroyed: false,
      locked: false,
      tool: "select",
      images: [],
      selectedIds: new Set<number>(),
      dragBefore: [],
      marqueeBaseIds: new Set<number>(),
      history: [],
      historyIndex: 0,
      canvas: fakeCanvas(),
      viewport: { centerX: 0, centerY: 0, height: 100 },
      cssHeight: 100,
      defaultWorldUnitsPerCssPixel: 1,
      strokeColor: "#ff0000",
      strokeWidthValue: 6,
      clearStrokePreview: vi.fn(),
      emitState: vi.fn(),
      refreshSelectionVisuals: vi.fn(),
      updateMarqueeOverlay: vi.fn(),
      onStrokeCommit: vi.fn(),
      ...options,
    });
  }

  it("refuses to switch tools while the board is locked", () => {
    const renderer = drawingRenderer({ locked: true });
    renderer.setTool("pen");
    expect((renderer as unknown as { tool: string }).tool).toBe("select");
  });

  it("drops selection and marquee state when a drawing tool is picked", () => {
    const selectedIds = new Set([3]);
    const marqueeBaseIds = new Set([3]);
    const renderer = drawingRenderer({ selectedIds, marqueeBaseIds });

    renderer.setTool("pen");

    expect((renderer as unknown as { tool: string }).tool).toBe("pen");
    expect(selectedIds.size).toBe(0);
    expect(marqueeBaseIds.size).toBe(0);
  });

  it("cancels an in-flight drawing instead of committing it", () => {
    const onStrokeCommit = vi.fn();
    const draft = beginStrokeDraft("pen", [0, 0]);
    extendStrokeDraft(draft, [50, 50], false);
    const renderer = drawingRenderer({
      draft,
      dragMode: "draw",
      dragPointerId: null,
      onStrokeCommit,
    });

    (renderer as unknown as { finishInteraction: () => void }).finishInteraction();

    // 锁定、切画板、失焦都走 finishActiveInteraction：没有落库就什么也不该发生。
    expect(onStrokeCommit).not.toHaveBeenCalled();
    expect((renderer as unknown as { draft: unknown }).draft).toBeNull();
    expect((renderer as unknown as { dragMode: unknown }).dragMode).toBeNull();
  });

  it("hands the encoded stroke to the controller and keeps the preview until it lands", () => {
    const onStrokeCommit = vi.fn();
    const draft = beginStrokeDraft("pen", [0, 0]);
    extendStrokeDraft(draft, [100, 50], false);
    const renderer = drawingRenderer({ draft, dragMode: "draw", onStrokeCommit });

    (renderer as unknown as { commitDrawing: () => void }).commitDrawing();

    expect(onStrokeCommit).toHaveBeenCalledTimes(1);
    const request = onStrokeCommit.mock.calls[0][0];
    expect(request.tool).toBe("pen");
    expect(request.color).toBe("#ff0000");
    expect(request.strokeWidth).toBe(6);
    // 墨迹外框：点中心极值 ± 半个线宽。
    expect(request.width).toBeCloseTo(106, 6);
    expect(request.height).toBeCloseTo(56, 6);
    expect(decodeStrokeHeader(request.geometry).tool).toBe(STROKE_TOOL.pen);
    // 落库要几个往返：预览留到新笔迹进缓冲，避免中途闪一下空白。
    expect((renderer as unknown as { dragMode: unknown }).dragMode).toBeNull();
    expect((renderer as unknown as { draft: unknown }).draft).not.toBeNull();

    renderer.clearPendingStrokePreview();
    expect((renderer as unknown as { draft: unknown }).draft).toBeNull();
  });

  it("keeps a new drawing's preview when the previous commit lands", () => {
    const draft = beginStrokeDraft("pen", [0, 0]);
    const renderer = drawingRenderer({ draft, dragMode: "draw" });

    renderer.clearPendingStrokePreview();

    // 正在画下一笔时不动：那一笔的预览已经取代了上一条。
    expect((renderer as unknown as { draft: unknown }).draft).toBe(draft);
  });

  it("treats a click without a drag as a cancelled stroke", () => {
    const onStrokeCommit = vi.fn();
    const draft = beginStrokeDraft("rect", [0, 0]);
    extendStrokeDraft(draft, [1, 1], false);
    const renderer = drawingRenderer({ draft, dragMode: "draw", onStrokeCommit });

    (renderer as unknown as { commitDrawing: () => void }).commitDrawing();

    // 1 个世界单位 = 1 CSS 像素，低于 3 像素的最小拖拽距离。
    expect(onStrokeCommit).not.toHaveBeenCalled();
  });
});
