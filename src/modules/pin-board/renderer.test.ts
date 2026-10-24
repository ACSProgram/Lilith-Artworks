import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { viewportBounds, type Viewport } from "./geometry";
import { PinBoardRenderer } from "./renderer";
import { pinBoardApi } from "./api";
import { isAdditiveSelectionModifier, marqueeSelection, sameSelection } from "./selection";
import {
  pendingSettleState,
  recordBoardRevision,
  resetPendingSettleForTests,
  resolveBoardRevision,
} from "./pendingSettle";
import type { PinBoardItem, PinBoardStrokeItem, PinBoardTextItem, PinBoardView } from "./types";
import { pinBoardTextSignature } from "./textEditing";
import {
  STROKE_TOOL,
  decodeStrokeHeader,
  type PinBoardStrokeGeometry,
} from "./strokeGeometry";
import { strokeGeometryFixture, strokeItemFixture } from "./pinBoardFixtures";
import { beginStrokeDraft, extendStrokeDraft } from "./strokeDrawing";
import { normalizeLayerVisibility, type PinBoardLayerVisibility } from "./layers";
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
    // 图层可见性：类字段初始化不会在替身对象上执行，这里补一份「全部显示」。
    visibleLayers: normalizeLayerVisibility(null),
    // 文字就地编辑的状态：类字段不会在替身对象上初始化。
    savedTextSignatures: new Map(),
    textEdit: null,
    placingText: false,
    textDefaults: { text: "", fontSize: 48, color: "#000000", align: "center" },
    textEditSessionId: 0,
    suppressClickEdit: false,
    pressClient: null,
    pressMoved: false,
    pressWasSelected: false,
    pressItemId: null,
    pressSuppressEdit: false,
    marqueeBaseIds: new Set(),
    dragBefore: [],
    dragWorld: [0, 0],
    // 橡皮擦的环形光标与擦除状态：同样是类字段初始化，替身要手工补齐。
    eraserRing: { hidden: true, style: {}, remove: vi.fn() },
    eraserPointer: null,
    strokeOutlineCache: new Map(),
    eraseHitIds: new Set(),
    eraseFrom: null,
    eraseCandidates: [],
    strokeWidthValue: 10,
    // 基准预算（低档）：与历史常量一致，预算相关的断言依赖这些值。
    textureBudgetBytes: PIN_BOARD_TEXTURE_RESIDENT_BUDGET_BYTES,
    previewTextureBudgetBytes: PIN_BOARD_PREVIEW_TEXTURE_BUDGET_BYTES,
    ...options,
  });
  return renderer;
}

/**
 * 原型上的 `emitState`。替身默认把它换成 mock（大部分用例只关心「有没有发出状态」），
 * 需要验证它自己的行为时（光标类名）用这个函数换回来——`emitState` 是私有方法，
 * 只能从原型上取。
 */
const realEmitState = (PinBoardRenderer.prototype as unknown as {
  emitState(this: PinBoardRenderer): void;
}).emitState;

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

  it("evicts only hi-res textures when the hi-res budget is exceeded, previews stay resident", () => {
    const renderer = rendererWith({
      textures: new Map([
        // 受保护的高清纹理贴着预算上限，另有一张可淘汰的高清纹理做淘汰对象。
        [1, makeLoaded(PIN_BOARD_TEXTURE_RESIDENT_BUDGET_BYTES - 4 * 1024 * 1024, 1)],
        [3, makeLoaded(8 * 1024 * 1024, 3)],
      ]),
      previewTextures: new Map([
        [2, makeLoaded(4 * 1024 * 1024, 2)],
      ]),
    });

    // 高清预算超限时只淘汰高清 textures，常驻缩略图不受影响。
    (renderer as unknown as {
      evictTextures: (protectedImageId?: number, additionalBytes?: number) => void;
    }).evictTextures(1, 16 * 1024 * 1024);

    expect((renderer as unknown as { textures: Map<number, unknown> }).textures.has(3)).toBe(false);
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
    return strokeGeometryFixture({
      itemId: 5,
      tool: STROKE_TOOL.line,
      strokeWidth: 2,
      width: 100,
      height: 100,
      color: "#FF8000",
    });
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

  it("rebuilds the stroke buffer when zoom changes or the visible set leaves the over-scan margin", () => {
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
      images: [strokeItem()],
      strokeGeometries: new Map([[5, strokeGeometry()]]),
      strokeVersion: 0,
      strokeSignature: "",
      strokeOutlineIndex: new Map(),
      strokeIndexCount: 0,
      gpu: { device },
      viewport: { centerX: 50, centerY: 50, height: 100 },
      canvas: { width: 100, height: 100 },
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

    // 平移在超扫边距以内：集合与缩放都没变，缓冲继续复用。
    (renderer as unknown as { viewport: { centerX: number } }).viewport.centerX = 60;
    syncStrokeBuffers();
    expect(created).toHaveLength(6);

    // 平移到笔迹完全落在「视口 + 边距」之外：集合变了，缓冲重建为空。
    (renderer as unknown as { viewport: { centerX: number } }).viewport.centerX = 10_000;
    syncStrokeBuffers();
    expect(created).toHaveLength(6);
    expect((renderer as unknown as { strokeIndexCount: number }).strokeIndexCount).toBe(0);
  });

  it("rebuilds after panning out of the over-scan box even when the item set is unchanged", () => {
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
    // 一条横跨整个画板的长笔迹：两种视口下它都「可见」，但缓冲里未必有远处那段。
    const wide = {
      ...strokeItem(),
      points: [[-1000, 0], [1000, 0], [1000, 100], [-1000, 100]],
      width: 2000,
    } as unknown as PinBoardItem;
    const renderer = rendererWith({
      images: [wide],
      strokeGeometries: new Map([[5, { ...strokeGeometry(), width: 2000 }]]),
      strokeOutlineIndex: new Map(),
      gpu: { device },
      viewport: { centerX: 50, centerY: 50, height: 100 },
      canvas: { width: 100, height: 100 },
      loaded: true,
      destroyed: false,
      boardId: 1,
      onError: vi.fn(),
    });
    const syncStrokeBuffers = () => (renderer as unknown as {
      syncStrokeBuffers: () => void;
    }).syncStrokeBuffers();

    syncStrokeBuffers();
    expect(created).toHaveLength(3);

    // 平移仍在构建缓冲时那个「视口 + 边距」框内：不重建。
    (renderer as unknown as { viewport: { centerX: number } }).viewport.centerX = 60;
    syncStrokeBuffers();
    expect(created).toHaveLength(3);

    // 条目集合没变，但新视口已在旧框之外：被裁掉的段可能露出来，必须重建。
    (renderer as unknown as { viewport: { centerX: number } }).viewport.centerX = 300;
    syncStrokeBuffers();
    expect(created).toHaveLength(6);
  });

  it("collects strokes from the over-scan margin, not just the viewport", () => {
    const near = strokeItem(5);
    const far = { ...strokeItem(9), points: strokeQuad.map(([x, y]) => [x + 10_000, y]) } as unknown as PinBoardItem;
    const device = {
      createBuffer: (descriptor: { size: number }) => ({
        getMappedRange: () => new ArrayBuffer(descriptor.size),
        unmap: () => undefined,
        destroy: () => undefined,
      }),
    };
    const renderer = rendererWith({
      images: [near, far],
      strokeGeometries: new Map([
        [5, strokeGeometry()],
        [9, { ...strokeGeometry(), itemId: 9 }],
      ]),
      strokeOutlineIndex: new Map(),
      gpu: { device },
      viewport: { centerX: 50, centerY: 50, height: 100 },
      canvas: { width: 100, height: 100 },
      loaded: true,
      destroyed: false,
      boardId: 1,
      onError: vi.fn(),
    });

    (renderer as unknown as { syncStrokeBuffers: () => void }).syncStrokeBuffers();

    const slots = (renderer as unknown as { strokeOutlineIndex: Map<number, number> }).strokeOutlineIndex;
    expect(slots.has(5)).toBe(true);
    // 一万单位之外的那条不参与构建：既省缓冲，也不会被逐段裁剪后的空图元拖慢。
    expect(slots.has(9)).toBe(false);
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
      gpu: {
        viewportBuffer: { destroy: vi.fn() },
        exportViewportBuffer: { destroy: vi.fn() },
      },
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

type LayerTestRenderer = {
  images: PinBoardItem[];
  selectedIds: Set<number>;
  visibleLayers: PinBoardLayerVisibility;
  visibleImages: PinBoardItem[];
  viewport: Viewport;
  pendingViewportFit: boolean;
  selectAll: () => void;
  selectedImages: () => PinBoardItem[];
  setLayerVisible: (layer: number, visible: boolean) => void;
  setSelectedLayer: (layer: 0 | 1 | 2) => void;
  bringToFront: (itemIds: number[]) => void;
  applyContentFit: (cssWidth: number, cssHeight: number) => void;
};

function layerItem(
  itemId: number,
  kind: "bitmap" | "stroke",
  layer: number,
  order: number,
  points: PinBoardItem["points"] = [[0, 0], [10, 0], [10, 10], [0, 10]],
): PinBoardItem {
  const base = {
    boardId: 1,
    itemId,
    width: 10,
    height: 10,
    order,
    layer,
    deleted: false,
    points,
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]] as PinBoardItem["uv"],
    available: true,
  };
  return kind === "stroke"
    ? { ...base, kind, tool: STROKE_TOOL.pen, color: "#ff0000", strokeWidth: 4 }
    : { ...base, kind };
}

function layerRenderer(options: Record<string, unknown>): LayerTestRenderer {
  const images = (options.images as PinBoardItem[] | undefined) ?? [];
  return rendererWith({
    loaded: true,
    locked: false,
    destroyed: false,
    images,
    imageById: new Map(images.map((image) => [image.itemId, image])),
    selectedIds: new Set<number>(),
    visibleLayers: normalizeLayerVisibility(null),
    visibleImages: [],
    dirty: false,
    savedStateKey: "",
    autosaveEnabled: false,
    defaultWorldUnitsPerCssPixel: 5,
    viewport: { centerX: 0, centerY: 0, height: 100 },
    refreshViewport: vi.fn(),
    emitState: vi.fn(),
    scheduleAutosave: vi.fn(),
    sortVisibleImages: vi.fn(),
    draw: vi.fn(),
    ...options,
  }) as unknown as LayerTestRenderer;
}

describe("pin-board layer visibility", () => {
  it("drops the items of a hidden layer from the selection", () => {
    const images = [layerItem(1, "bitmap", 1, 0), layerItem(2, "stroke", 3, 1)];
    const renderer = layerRenderer({
      images,
      selectedIds: new Set<number>([1, 2]),
    });

    renderer.setLayerVisible(3, false);

    expect([...renderer.selectedIds]).toEqual([1]);
    expect(renderer.visibleLayers).toEqual([true, true, true, false]);
  });

  it("keeps the selection when a layer is turned back on", () => {
    const renderer = layerRenderer({
      images: [layerItem(1, "bitmap", 1, 0)],
      selectedIds: new Set<number>([1]),
      visibleLayers: normalizeLayerVisibility([true, false, true, true]),
    });

    renderer.setLayerVisible(1, true);

    expect([...renderer.selectedIds]).toEqual([1]);
  });

  it("ignores toggles that would not change anything or are out of range", () => {
    const refreshViewport = vi.fn();
    const renderer = layerRenderer({ images: [], refreshViewport });

    renderer.setLayerVisible(1, true);
    renderer.setLayerVisible(9, false);

    expect(refreshViewport).not.toHaveBeenCalled();
  });

  it("leaves hidden layers out of select-all", () => {
    const renderer = layerRenderer({
      images: [
        layerItem(1, "bitmap", 1, 0),
        layerItem(2, "bitmap", 0, 1),
        layerItem(3, "stroke", 3, 2),
      ],
      visibleLayers: normalizeLayerVisibility([true, true, true, false]),
    });

    renderer.selectAll();

    expect([...renderer.selectedIds].sort((left, right) => left - right)).toEqual([1, 2]);
  });

  it("does not count hidden items as selected", () => {
    const renderer = layerRenderer({
      images: [layerItem(1, "bitmap", 1, 0), layerItem(2, "bitmap", 0, 1)],
      selectedIds: new Set<number>([1, 2]),
      visibleLayers: normalizeLayerVisibility([true, true, true, true]),
    });

    expect(renderer.selectedImages().map((image) => image.itemId)).toEqual([1, 2]);

    (renderer as unknown as { visibleLayers: PinBoardLayerVisibility }).visibleLayers =
      normalizeLayerVisibility([false, true, true, true]);
    expect(renderer.selectedImages().map((image) => image.itemId)).toEqual([1]);
  });

  it("fits the view without the hidden layers", () => {
    const renderer = layerRenderer({
      images: [
        layerItem(1, "bitmap", 1, 0, [[0, 300], [400, 300], [400, 0], [0, 0]]),
        // 隐藏层里的条目必须排除，否则「隐藏标注层 → 重置视图」会缩放到一片空白。
        layerItem(2, "stroke", 3, 1, [[9000, 9300], [9400, 9300], [9400, 9000], [9000, 9000]]),
      ],
      visibleLayers: normalizeLayerVisibility([true, true, true, false]),
      pendingViewportFit: true,
    });

    renderer.applyContentFit(1000, 600);

    expect(renderer.pendingViewportFit).toBe(false);
    expect(renderer.viewport.centerX).toBe(200);
    expect(renderer.viewport.centerY).toBe(150);
  });

  it("refuses to move a mixed selection to another layer", () => {
    const images = [layerItem(1, "bitmap", 1, 0), layerItem(2, "stroke", 3, 1)];
    const renderer = layerRenderer({ images, selectedIds: new Set<number>([1, 2]) });

    renderer.setSelectedLayer(2);

    // 混选整组拒绝：只改图片会让用户以为笔迹也跟着换了层。
    expect(renderer.images.map((image) => image.layer)).toEqual([1, 3]);
  });
});

describe("pin-board stroke ordering", () => {
  it("promotes a stroke only among strokes", () => {
    const images = [
      layerItem(1, "bitmap", 1, 0),
      layerItem(2, "bitmap", 1, 1),
      layerItem(3, "stroke", 3, 2),
      layerItem(4, "stroke", 3, 3),
    ];
    const renderer = layerRenderer({ images });

    renderer.bringToFront([3]);

    // 图片的 sort_order 一个都不动：跨层先后由 layer 决定，改它只是脏整块画板。
    expect(images.map((image) => [image.itemId, image.order])).toEqual([
      [1, 0],
      [2, 1],
      [3, 2],
      [4, 3],
    ]);

    renderer.bringToFront([4]);
    expect(images.map((image) => [image.itemId, image.order])).toEqual([
      [1, 0],
      [2, 1],
      [3, 3],
      [4, 2],
    ]);
  });

  it("still promotes an image above the other images", () => {
    const images = [
      layerItem(1, "bitmap", 1, 0),
      layerItem(2, "bitmap", 1, 1),
      layerItem(3, "stroke", 3, 2),
    ];
    const renderer = layerRenderer({ images });

    renderer.bringToFront([2]);

    expect(images.map((image) => [image.itemId, image.order])).toEqual([
      [1, 1],
      [2, 0],
      [3, 2],
    ]);
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

  it("lets the caller replace the quad test with an ink test", () => {
    const images = [boardImage(1, topLeft), boardImage(2, bottomRight), boardImage(3, topLeft, true)];
    const hitTest = vi.fn<(image: PinBoardItem) => boolean>(() => false);

    // 谓词只在通过删除/可用性检查之后调用：已删除的条目根本不参与判定。
    expect([...marqueeSelection(images, topLeftBounds, [], hitTest)]).toEqual([]);
    expect(hitTest).toHaveBeenCalledTimes(2);
    expect(hitTest.mock.calls.map(([image]) => image.itemId)).toEqual([1, 2]);
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
  interactionState: { dirty: boolean; itemCount: number };
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
    expect(renderer.interactionState.itemCount).toBe(2);
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

  it("switches the canvas cursor class with the pen tool", () => {
    const toggle = vi.fn();
    const renderer = drawingRenderer({
      canvas: { ...fakeCanvas(), classList: { add: vi.fn(), remove: vi.fn(), toggle } },
      // 真实的 emitState：光标类名就在里面切换，mock 掉就什么也测不到。
      emitState: realEmitState,
      updateSelectionOverlay: vi.fn(),
      persistSession: vi.fn(),
      onState: vi.fn(),
    });

    renderer.setTool("pen");
    expect(toggle).toHaveBeenCalledWith("tool-pen", true);

    // 橡皮有自己的类名（用来藏掉十字光标），笔尖类名要同时撤掉。
    renderer.setTool("eraser");
    expect(toggle).toHaveBeenCalledWith("tool-pen", false);
    expect(toggle).toHaveBeenCalledWith("tool-eraser", true);

    // 其余工具保持十字：两个类名都撤掉即可，不需要逐工具写光标。
    renderer.setTool("rect");
    expect(toggle).toHaveBeenLastCalledWith("tool-eraser", false);
    expect(toggle).toHaveBeenCalledWith("tool-pen", false);
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

/** 擦除、复制载荷与图层计数共用同一套「已装载的小画板」替身，因此归在一个 describe 里。 */
describe("pin-board eraser, clipboard payload and layer counts", () => {
  const ERASER_STROKE_WIDTH = 10;

  function quad(left: number): PinBoardItem["points"] {
    return [[left, 0], [left + 100, 0], [left + 100, 100], [left, 100]];
  }

  function strokeItem(itemId: number, left = 0, layer = 3): PinBoardItem {
    return {
      boardId: 1,
      itemId,
      kind: "stroke",
      width: 100,
      height: 100,
      order: itemId,
      layer,
      deleted: false,
      points: quad(left),
      uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
      available: true,
      tool: STROKE_TOOL.line,
      color: "#FF8000",
      strokeWidth: ERASER_STROKE_WIDTH,
    } as unknown as PinBoardItem;
  }

  function bitmapItem(itemId: number, left = 0): PinBoardItem {
    return {
      boardId: 1,
      itemId,
      kind: "bitmap",
      width: 100,
      height: 100,
      order: itemId,
      layer: 1,
      deleted: false,
      points: quad(left),
      uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
      available: true,
    } as unknown as PinBoardItem;
  }

  function textItem(itemId: number): PinBoardItem {
    return {
      ...bitmapItem(itemId),
      kind: "text",
      text: "备注",
      fontFamily: "sans-serif",
      fontSize: 14,
      color: "#ffffff",
      align: "left",
      bold: false,
      italic: false,
    } as unknown as PinBoardItem;
  }

  function lineGeometry(itemId: number): PinBoardStrokeGeometry {
    return strokeGeometryFixture({
      itemId,
      tool: STROKE_TOOL.line,
      strokeWidth: ERASER_STROKE_WIDTH,
      width: 100,
      height: 100,
      color: "#FF8000",
    });
  }

  function eraserRenderer(images: PinBoardItem[], options: Record<string, unknown> = {}) {
    return rendererWith({
      loaded: true,
      destroyed: false,
      locked: false,
      tool: "eraser",
      images,
      imageById: new Map(images.map((image) => [image.itemId, image])),
      selectedIds: new Set<number>(),
      dragMode: null,
      dragPointerId: 1,
      dragBefore: [],
      marqueeBaseIds: new Set<number>(),
      history: [],
      historyIndex: 0,
      dirty: false,
      saving: false,
      strokeWidthValue: ERASER_STROKE_WIDTH,
      defaultWorldUnitsPerCssPixel: 1,
      worldUnitsPerCssPixel: () => 1,
      strokeGeometries: new Map(
        images
          .filter((image) => image.kind === "stroke")
          .map((image) => [image.itemId, lineGeometry(image.itemId)]),
      ),
      canvas: {
        classList: { add: vi.fn(), remove: vi.fn() },
        hasPointerCapture: () => false,
        releasePointerCapture: vi.fn(),
        getBoundingClientRect: () => ({ width: 100, height: 100, left: 0, top: 0 }),
      },
      viewport: { centerX: 50, centerY: 50, height: 100 },
      refreshSelectionVisuals: vi.fn(),
      syncGeometryBuffers: vi.fn(),
      refreshViewport: vi.fn(),
      emitState: vi.fn(),
      stateKey: () => "current",
      savedStateKey: "saved",
      scheduleAutosave: vi.fn(),
      cancelAutosave: vi.fn(),
      ...options,
    });
  }

  type EraserTestRenderer = {
    beginErasing: (world: [number, number]) => boolean;
    eraseAt: (world: [number, number]) => number[];
    finishInteraction: () => void;
    interactionState: { layerCounts: number[] };
    copySelection: () => unknown[];
    dragMode: unknown;
    eraseHitIds: Set<number>;
    dirty: boolean;
    locked: boolean;
    history: {
      before: { itemId: number; deleted: boolean }[];
      after: { itemId: number; deleted: boolean }[];
    }[];
  };

  it("erases every stroke under one drag and commits a single history entry", () => {
    const first = strokeItem(11);
    const second = strokeItem(12, 200);
    const renderer = eraserRenderer([first, second]) as unknown as EraserTestRenderer;

    expect(renderer.beginErasing([50, 50])).toBe(true);
    expect(first.deleted).toBe(true);
    expect(second.deleted).toBe(false);
    expect(renderer.dragMode).toBe("erase");

    // 继续拖动：只处理新命中的条目，两条最终落在同一条撤销记录里。
    expect(renderer.eraseAt([250, 50])).toEqual([12]);
    renderer.finishInteraction();

    expect(renderer.history).toHaveLength(1);
    expect(renderer.history[0].before.map((snapshot) => snapshot.deleted)).toEqual([false, false]);
    expect(renderer.history[0].after.map((snapshot) => snapshot.deleted)).toEqual([true, true]);
    expect(renderer.dirty).toBe(true);
    expect(renderer.eraseHitIds.size).toBe(0);
  });

  it("erases along the swept path between two pointer samples", () => {
    const stroke = strokeItem(11);
    const renderer = eraserRenderer([stroke]) as unknown as EraserTestRenderer;

    // 两个采样点都离笔迹很远（笔迹是从 (5,5) 到 (95,95) 的对角线），但连线正好穿过它。
    // 只判当前点会漏掉这种「快速擦过细线」的情形，这也是橡皮时灵时不灵的根源。
    expect(renderer.beginErasing([-40, 50])).toBe(true);
    expect(stroke.deleted).toBe(false);

    expect(renderer.eraseAt([100, 50])).toEqual([11]);
    renderer.finishInteraction();

    expect(renderer.history).toHaveLength(1);
    expect(renderer.history[0].after.map((snapshot) => snapshot.deleted)).toEqual([true]);
  });

  it("does not erase when the swept path misses the ink", () => {
    const stroke = strokeItem(11);
    const renderer = eraserRenderer([stroke]) as unknown as EraserTestRenderer;

    expect(renderer.beginErasing([-40, -40])).toBe(true);
    // 与上一点同在笔迹左上方，整段都不碰墨迹。
    expect(renderer.eraseAt([0, -10])).toEqual([]);
    renderer.finishInteraction();

    expect(stroke.deleted).toBe(false);
    expect(renderer.history).toHaveLength(0);
  });

  it("keeps the board untouched when the click hits no ink", () => {
    const stroke = strokeItem(11);
    const renderer = eraserRenderer([stroke]) as unknown as EraserTestRenderer;

    // 没有命中也要进入拖拽：按下后继续拖可能才碰到笔迹。
    expect(renderer.beginErasing([500, 500])).toBe(true);
    expect(stroke.deleted).toBe(false);

    renderer.finishInteraction();

    expect(renderer.history).toHaveLength(0);
    expect(renderer.dirty).toBe(false);
    expect(renderer.dragMode).toBeNull();
  });

  it("never erases bitmaps and skips strokes on hidden layers", () => {
    const stroke = strokeItem(11);
    const bitmap = bitmapItem(12);
    const renderer = eraserRenderer([stroke, bitmap], {
      visibleLayers: normalizeLayerVisibility([true, true, true, false]),
    }) as unknown as EraserTestRenderer;

    // 标注层被隐藏：看不见的笔迹不该被擦掉，位图本来就不参与擦除。
    expect(renderer.beginErasing([50, 50])).toBe(false);
    expect(stroke.deleted).toBe(false);
    expect(bitmap.deleted).toBe(false);
  });

  it("records only the source of each clipboard item", () => {
    const renderer = eraserRenderer(
      [bitmapItem(1), textItem(2), strokeItem(3)],
      { tool: "select", selectedIds: new Set([1, 2, 3]) },
    ) as unknown as EraserTestRenderer;

    // 三类条目都只带「来源」：尺寸、变换与载荷由原生端读源条目现算。
    expect(renderer.copySelection()).toEqual([
      { kind: "bitmap", sourceBoardId: 1, sourceItemId: 1 },
      { kind: "text", sourceBoardId: 1, sourceItemId: 2 },
      { kind: "stroke", sourceBoardId: 1, sourceItemId: 3 },
    ]);

    renderer.locked = true;
    expect(renderer.copySelection()).toEqual([]);
  });

  it("counts items per layer without dropping hidden ones", () => {
    const renderer = eraserRenderer(
      [
        { ...bitmapItem(1, 0), layer: 0 },
        { ...bitmapItem(2, 0), layer: 1 },
        { ...bitmapItem(3, 0), layer: 1, deleted: true },
        { ...textItem(4), layer: 1 },
        strokeItem(5),
      ],
      {
        tool: "select",
        visibleLayers: normalizeLayerVisibility([true, true, true, false]),
      },
    ) as unknown as EraserTestRenderer;

    // 隐藏的标注层照样计入；已删除的条目不计入。
    expect(renderer.interactionState.layerCounts).toEqual([1, 2, 0, 1]);
  });
});

describe("pin-board ink hit dispatch", () => {
  const QUAD: PinBoardItem["points"] = [[0, 0], [100, 0], [100, 100], [0, 100]];

  // 与 `strokeHit.test.ts` 共用同一份条目：这里只验证渲染器的分派，几何判定留在那里。
  function strokeItem(
    itemId: number,
    overrides: Partial<PinBoardStrokeItem> = {},
  ): PinBoardItem {
    return strokeItemFixture({
      itemId,
      width: 100,
      height: 100,
      order: itemId,
      points: QUAD,
      tool: STROKE_TOOL.rect,
      color: "#FF8000",
      strokeWidth: 4,
      ...overrides,
    }) as unknown as PinBoardItem;
  }

  /** 世界单位 / CSS 像素 = 1（viewport 与 cssHeight 都是 100），容差即 3 个世界单位。 */
  function inkRenderer(images: PinBoardItem[], geometries: PinBoardStrokeGeometry[]) {
    return rendererWith({
      loaded: true,
      destroyed: false,
      images,
      imageById: new Map(images.map((image) => [image.itemId, image])),
      visibleImages: images,
      strokeGeometries: new Map(geometries.map((geometry) => [geometry.itemId, geometry])),
      viewport: { centerX: 50, centerY: 50, height: 100 },
      cssHeight: 100,
    }) as unknown as {
      hitImage: (point: [number, number]) => PinBoardItem | null;
      itemIntersectsMarquee: (
        image: PinBoardItem,
        bounds: { minX: number; minY: number; maxX: number; maxY: number },
      ) => boolean;
    };
  }

  it("keeps bitmaps on their bounding quad", () => {
    const bitmap = { ...strokeItem(2), kind: "bitmap", layer: 1 } as PinBoardItem;
    const renderer = inkRenderer([bitmap], []);

    // 位图不读回纹理就无法知道 alpha，中心空白也按框判。
    expect(renderer.hitImage([50, 50])).toBe(bitmap);
    expect(renderer.hitImage([150, 50])).toBeNull();
  });

  it("falls back to the quad while stroke geometry is still loading", () => {
    const rect = strokeItem(5);
    const renderer = inkRenderer([rect], []);

    expect(renderer.hitImage([50, 50])).toBe(rect);
    expect(renderer.itemIntersectsMarquee(rect, { minX: 20, minY: 20, maxX: 80, maxY: 80 })).toBe(true);
  });
});

describe("pin-board text inline editing", () => {
  const measureContext = {
    font: "",
    measureText: (line: string) => ({
      width: line.length * 10,
      actualBoundingBoxAscent: 8,
      actualBoundingBoxDescent: 2,
    }),
  };

  function textItem(overrides: Partial<PinBoardTextItem> = {}): PinBoardTextItem {
    return {
      kind: "text",
      boardId: 9,
      itemId: 7,
      width: 20,
      height: 26,
      order: 0,
      layer: 1,
      deleted: false,
      points: [[0, 0], [40, 0], [40, 52], [0, 52]],
      uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
      available: true,
      text: "ab",
      fontFamily: '"Microsoft YaHei UI", "Segoe UI", sans-serif',
      fontSize: 20,
      color: "#112233",
      align: "center",
      ...overrides,
    };
  }

  function fakeCanvas() {
    return {
      classList: { add: vi.fn(), remove: vi.fn(), toggle: vi.fn() },
      hasPointerCapture: () => false,
      releasePointerCapture: vi.fn(),
      focus: vi.fn(),
      getBoundingClientRect: () => ({ width: 100, height: 100, left: 0, top: 0 }),
    };
  }

  function textRenderer(images: PinBoardTextItem[], options: Record<string, unknown> = {}) {
    return rendererWith({
      loaded: true,
      destroyed: false,
      locked: false,
      exporting: false,
      gpuUnavailable: false,
      active: true,
      tool: "select",
      boardId: 9,
      revision: "r1",
      images,
      imageById: new Map(images.map((image) => [image.itemId, image])),
      selectedIds: new Set(images.map((image) => image.itemId)),
      savedTextSignatures: new Map(images.map((image) => [image.itemId, pinBoardTextSignature(image)])),
      history: [],
      historyIndex: 0,
      savedStateKey: "",
      dirty: false,
      autosaveTimer: null,
      autosaveEnabled: false,
      canvas: fakeCanvas(),
      viewport: { centerX: 0, centerY: 0, height: 100 },
      cssHeight: 100,
      defaultWorldUnitsPerCssPixel: 1,
      textures: new Map(),
      previewTextures: new Map(),
      textureQualityCaps: new Map(),
      failedTextureDimensions: new Map(),
      refreshViewport: vi.fn(),
      updateSelectionOverlay: vi.fn(),
      draw: vi.fn(),
      emitState: vi.fn(),
      onError: vi.fn(),
      scheduleAutosave: vi.fn(),
      ...options,
    });
  }

  beforeEach(() => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext")
      .mockReturnValue(measureContext as unknown as CanvasRenderingContext2D);
    resetPendingSettleForTests();
  });

  it("applies a draft, rebuilds the quad and records one undo entry", () => {
    const image = textItem();
    const renderer = textRenderer([image]);

    expect(renderer.beginTextEdit(7)).toBe(true);
    const state = renderer.interactionState.textEditing;
    expect(state?.draft).toEqual({ text: "ab", fontSize: 20, color: "#112233", align: "center" });
    expect(state?.scale).toBe(2);

    renderer.updateTextEditDraft({ text: "abcd", fontSize: 24, color: "#AABBCC", align: "left" });
    expect(renderer.commitTextEdit()).toBeNull();

    expect(image.text).toBe("abcd");
    expect(image.fontSize).toBe(24);
    expect(image.color).toBe("#aabbcc");
    expect(image.align).toBe("left");
    // 测量宽 = 4 字 × 10，高 = ceil(24 × 1.3)；条目缩放仍是 2 倍。
    expect(image.width).toBe(40);
    expect(image.height).toBe(32);
    expect(Math.hypot(
      image.points[1][0] - image.points[0][0],
      image.points[1][1] - image.points[0][1],
    )).toBeCloseTo(80, 10);
    const history = (renderer as unknown as { history: { before: unknown[]; after: unknown[] }[] }).history;
    expect(history).toHaveLength(1);
    expect(history[0].before[0]).toMatchObject({ itemId: 7, text: expect.objectContaining({ text: "ab" }) });
    expect(history[0].after[0]).toMatchObject({ itemId: 7, text: expect.objectContaining({ text: "abcd" }) });
    expect(renderer.interactionState.textEditing).toBeNull();
  });

  it("cancels an emptied draft instead of deleting the text", () => {
    const image = textItem();
    const renderer = textRenderer([image]);

    renderer.beginTextEdit(7);
    renderer.updateTextEditDraft({ text: "   ", fontSize: 20, color: "#112233", align: "center" });
    expect(renderer.commitTextEdit()).toBeNull();

    expect(image.text).toBe("ab");
    expect(renderer.interactionState.textEditing).toBeNull();
    expect((renderer as unknown as { history: unknown[] }).history).toHaveLength(0);
  });

  it("closes the session without touching the model when nothing changed", () => {
    const image = textItem({ points: [[10, 20], [50, 20], [50, 72], [10, 72]] });
    const renderer = textRenderer([image]);

    expect(renderer.beginTextEdit(7)).toBe(true);
    expect(renderer.commitTextEdit()).toBeNull();

    // 重建四边形会引入浮点误差：没有实际改动时不该写历史、也不该把条目记成已改。
    expect(image.points).toEqual([[10, 20], [50, 20], [50, 72], [10, 72]]);
    expect((renderer as unknown as { history: unknown[] }).history).toHaveLength(0);
    expect(renderer.interactionState.dirty).toBe(false);
    expect(renderer.interactionState.textEditing).toBeNull();
  });

  it("keeps the session and returns an error when the draft is invalid", () => {
    const image = textItem();
    const renderer = textRenderer([image]);

    renderer.beginTextEdit(7);
    renderer.updateTextEditDraft({ text: "x".repeat(820), fontSize: 20, color: "#112233", align: "center" });

    const error = renderer.commitTextEdit();
    expect(error).toContain("超出素材板上限");
    expect(renderer.interactionState.textEditing).not.toBeNull();
    expect(image.text).toBe("ab");
  });

  it("sends only the text payloads whose signature changed", () => {
    const first = textItem({ itemId: 1 });
    const second = textItem({ itemId: 2, text: "cd" });
    const renderer = textRenderer([first, second]);
    first.text = "changed";

    const editable = (renderer as unknown as {
      editableImages: () => { itemId: number; text?: { text: string } }[];
    }).editableImages();

    expect(editable.find((item) => item.itemId === 1)?.text?.text).toBe("changed");
    expect(editable.find((item) => item.itemId === 2)?.text).toBeUndefined();
  });

  it("counts text-only changes in the dirty state key", () => {
    const image = textItem();
    const renderer = textRenderer([image]);
    const stateKey = (renderer as unknown as { stateKey: () => string }).stateKey.bind(renderer);
    const before = stateKey();

    image.text = "changed";

    expect(stateKey()).not.toBe(before);
  });

  it("restores text from a snapshot and invalidates its textures", () => {
    const image = textItem();
    const loaded = makeLoaded(64, 0);
    const preview = makeLoaded(16, 0);
    const textures = new Map([[7, loaded]]);
    const previewTextures = new Map([[7, preview]]);
    const textureQualityCaps = new Map([[7, 512]]);
    const failedTextureDimensions = new Map([[7, 256]]);
    const renderer = textRenderer([image], {
      textures,
      previewTextures,
      textureQualityCaps,
      failedTextureDimensions,
    });
    const restore = (renderer as unknown as {
      restore: (snapshots: unknown[]) => void;
    }).restore.bind(renderer);

    restore([{
      itemId: 7,
      deleted: false,
      layer: 1,
      points: [[0, 0], [40, 0], [40, 52], [0, 52]],
      uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
      text: { text: "undo", fontSize: 20, color: "#112233", align: "center", width: 40, height: 26 },
    }]);

    expect(image.text).toBe("undo");
    expect(image.width).toBe(40);
    expect(textures.size).toBe(0);
    expect(previewTextures.size).toBe(0);
    expect(textureQualityCaps.size).toBe(0);
    expect(failedTextureDimensions.size).toBe(0);
    expect(loaded.texture.destroy).toHaveBeenCalledTimes(3);
    expect(preview.texture.destroy).toHaveBeenCalledTimes(3);
  });

  it("advances saved signatures only to the values present when the save started", async () => {
    const image = textItem();
    const renderer = textRenderer([image]);
    recordBoardRevision(9, "r1");
    let finishSave!: (value: { saved: boolean; revision: string; nowStep: number }) => void;
    vi.spyOn(pinBoardApi, "savePinBoard").mockReturnValue(new Promise((resolve) => {
      finishSave = resolve;
    }));

    image.text = "first";
    const saving = renderer.save();
    // 保存期间又改了一次：请求里带的是 first，落地后不能把这个改动记成已保存。
    image.text = "second";
    finishSave({ saved: true, revision: "r2", nowStep: 2 });
    await saving;

    expect(renderer.interactionState.dirty).toBe(true);
    const signatures = (renderer as unknown as { savedTextSignatures: Map<number, string> })
      .savedTextSignatures;
    expect(signatures.get(7)).toBe(pinBoardTextSignature(textItem({ text: "first" })));
    expect(signatures.get(7)).not.toBe(pinBoardTextSignature(image));
  });

  it("enters editing only on a click over the already-selected single text", () => {
    const beginTextEdit = vi.fn();
    const renderer = textRenderer([textItem()], {
      beginTextEdit,
      updateMarqueeOverlay: vi.fn(),
      resetEraseState: vi.fn(),
    });
    const finish = (press: {
      moved: boolean;
      selectedCount: number;
    }) => {
      Object.assign(renderer, {
        dragMode: "images",
        dragBefore: [],
        dragPointerId: null,
        pressMoved: press.moved,
        pressSuppressEdit: false,
        pressWasSelected: true,
        pressItemId: 7,
        selectedIds: new Set(
          press.selectedCount > 1 ? [7, 8] : [7],
        ),
      });
      (renderer as unknown as { finishInteraction: (allow?: boolean) => void }).finishInteraction();
    };

    finish({ moved: false, selectedCount: 1 });
    // 按下点随进入编辑一起传下去，用来把光标落到点击处。
    expect(beginTextEdit).toHaveBeenCalledWith(7, [0, 0]);

    beginTextEdit.mockClear();
    finish({ moved: true, selectedCount: 1 });
    expect(beginTextEdit).not.toHaveBeenCalled();

    finish({ moved: false, selectedCount: 2 });
    expect(beginTextEdit).not.toHaveBeenCalled();
  });

  it("opens a blank draft at the click point while placing text", () => {
    const renderer = textRenderer([], {
      refreshSelectionVisuals: vi.fn(),
      updateMarqueeOverlay: vi.fn(),
      onContextMenu: vi.fn(),
    });

    expect(renderer.startTextPlacement()).toBe(true);
    expect(renderer.interactionState.placingText).toBe(true);
    renderer.cancelTextPlacement();
    expect(renderer.interactionState.placingText).toBe(false);

    // 落点由 `pointerDown` 换算成世界坐标后交给这里；两种进入方式共用同一段开框逻辑。
    (renderer as unknown as {
      beginNewTextEdit: (center: [number, number]) => void;
    }).beginNewTextEdit([0, 0]);

    const editing = renderer.interactionState.textEditing;
    expect(editing?.isNew).toBe(true);
    expect(editing?.itemId).toBeNull();
    expect(editing?.caret).toBe(0);
    expect(editing?.draft).toEqual({
      text: "",
      fontSize: 48,
      color: "#000000",
      align: "center",
    });
    const defaults = (renderer as unknown as {
      textDefaults: { fontSize: number; color: string; align: string };
    }).textDefaults;
    expect(defaults.fontSize).toBe(48);
    renderer.updateTextEditDraft({ text: "hi", fontSize: 64, color: "#AABBCC", align: "left" });
    const resolved = renderer.resolveNewTextEdit();
    expect(resolved).toMatchObject({
      ok: true,
      request: {
        text: "hi",
        fontSize: 64,
        color: "#aabbcc",
        align: "left",
        measuredWidth: 20,
        measuredHeight: 84,
      },
    });

    renderer.completeNewTextEdit();
    expect(renderer.interactionState.textEditing).toBeNull();
    // 下一次新建沿用这次提交的样式。
    const nextDefaults = (renderer as unknown as {
      textDefaults: { fontSize: number; color: string; align: string };
    }).textDefaults;
    expect(nextDefaults).toEqual({ text: "", fontSize: 64, color: "#aabbcc", align: "left" });
  });

  it("cancels text placement without opening a menu", () => {
    const renderer = textRenderer([], {
      refreshSelectionVisuals: vi.fn(),
      updateMarqueeOverlay: vi.fn(),
      onContextMenu: vi.fn(),
    });
    renderer.startTextPlacement();

    renderer.cancelTextPlacement();

    expect(renderer.interactionState.placingText).toBe(false);
    expect(renderer.interactionState.textEditing).toBeNull();
  });

  it("opens the blank draft at the right-click point without placement mode", () => {
    const onContextMenu = vi.fn();
    const renderer = textRenderer([], {
      refreshSelectionVisuals: vi.fn(),
      updateMarqueeOverlay: vi.fn(),
      onContextMenu,
    });

    // 菜单项带自己的落点：画布 100×100、视口高 100 时右下角对应世界坐标 (50, 50)。
    expect(renderer.addTextAtClientPoint(100, 0)).toBe(true);

    const state = renderer.interactionState;
    expect(state.placingText).toBe(false);
    expect(state.textEditing?.isNew).toBe(true);
    const edit = (renderer as unknown as { textEdit: { center: [number, number] } }).textEdit;
    expect(edit.center).toEqual([50, 50]);
    // 编辑框回到点击处：屏幕坐标应与右键位置一致。
    expect(state.textEditing?.centerX).toBeCloseTo(100, 10);
    expect(state.textEditing?.centerY).toBeCloseTo(0, 10);
    expect(onContextMenu).toHaveBeenCalledWith(null);
  });

  it("zooms the view in when the text would be too small to edit", () => {
    const image = textItem({
      fontSize: 1,
      width: 10,
      height: 2,
      points: [[0, 0], [10, 0], [10, 2], [0, 2]],
    });
    const renderer = textRenderer([image]);

    expect(renderer.beginTextEdit(7)).toBe(true);

    // 1 世界单位 = 1 CSS 像素，屏幕字号只有 1 像素：视图放大到下限（视口高 20）。
    expect((renderer as unknown as { viewport: { height: number } }).viewport.height).toBe(20);
    // 只改观看比例，数据库字号与字体栏显示的有效字号都不变。
    expect(renderer.interactionState.textEditing?.draft.fontSize).toBe(1);
    expect(renderer.interactionState.textEditing?.scale).toBe(1);
  });

  it("puts the caret where the second click landed", () => {
    const image = textItem({ text: "ab", width: 20, fontSize: 20 });
    const renderer = textRenderer([image]);

    expect(renderer.beginTextEdit(7, [30, 0])).toBe(true);
    expect(renderer.interactionState.textEditing?.caret).toBe(1);
  });
});
