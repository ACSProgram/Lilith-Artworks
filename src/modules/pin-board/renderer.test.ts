import { afterEach, describe, expect, it, vi } from "vitest";
import { viewportBounds, type Viewport } from "./geometry";
import { PinBoardRenderer } from "./renderer";
import type { PinBoardImage } from "./types";
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
    // 基准预算（低档）：与历史常量一致，预算相关的断言依赖这些值。
    textureBudgetBytes: PIN_BOARD_TEXTURE_RESIDENT_BUDGET_BYTES,
    previewTextureBudgetBytes: PIN_BOARD_PREVIEW_TEXTURE_BUDGET_BYTES,
    ...options,
  });
  return renderer;
}

function boardImage(imageId: number, points: PinBoardImage["points"], deleted = false): PinBoardImage {
  return {
    boardId: 1,
    imageId,
    width: 100,
    height: 100,
    order: imageId,
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
    const points = [[0, 0], [400, 0], [400, 300], [0, 300]] as unknown as PinBoardImage["points"];
    const image = {
      imageId: 7,
      width: 4000,
      height: 3000,
      points,
    } as unknown as PinBoardImage;
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
    const points = [[0, 0], [400, 0], [400, 300], [0, 300]] as unknown as PinBoardImage["points"];
    const image = {
      imageId: 7,
      width: 4000,
      height: 3000,
      points,
    } as unknown as PinBoardImage;
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
  afterModelChange: (imageIds: number[]) => void;
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

type SelectionTestRenderer = {
  selectedIds: Set<number>;
  selectAll: () => void;
};

describe("pin-board renderer select-all", () => {
  const quad: PinBoardImage["points"] = [[0, 0], [10, 0], [10, 10], [0, 10]];

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
