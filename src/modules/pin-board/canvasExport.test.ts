import { afterEach, describe, expect, it, vi } from "vitest";
import { pinBoardApi } from "./api";
import { normalizeLayerVisibility } from "./layers";
import { PinBoardRenderer } from "./renderer";
import type { PinBoardItem } from "./types";

/**
 * 整画布导出的渲染通道测试。
 *
 * 测试环境没有真实 WebGPU 适配器，替身设备只实现导出路径真正调用的那些方法：这里
 * 断言的是「块的顺序、尺寸、字节数与命令调用」，像素内容由 Rust 侧的 PNG 单测覆盖。
 */

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function rendererWith(options: Record<string, unknown>): PinBoardRenderer {
  const renderer = Object.create(PinBoardRenderer.prototype) as PinBoardRenderer;
  Object.assign(renderer, {
    images: [],
    textures: new Map(),
    previewTextures: new Map(),
    failedTextureDimensions: new Map(),
    visibleImages: [],
    visibleLayers: normalizeLayerVisibility(null),
    selectedIds: new Set(),
    history: [],
    historyIndex: 0,
    locked: false,
    saving: false,
    dirty: false,
    tool: "select",
    viewport: { centerX: 0, centerY: 0, height: 1000 },
    cssHeight: 500,
    defaultWorldUnitsPerCssPixel: 1,
    strokeGeometries: new Map(),
    strokeOutlineIndex: new Map(),
    strokeVersion: 0,
    strokeSignature: "",
    strokeLoadToken: 0,
    strokeIndexCount: 0,
    textureReservedBytes: 0,
    textureBudgetBytes: 256 * 1024 * 1024,
    exportDrawItems: [],
    exportStrokeVertexBuffer: null,
    exportStrokeIndexBuffer: null,
    exportStrokeIndexCount: 0,
    ...options,
  });
  return renderer;
}

/** 只实现导出路径用到的方法，并记录渲染 pass 与回读调用。 */
function exportGpu() {
  const pass = {
    setViewport: vi.fn(),
    setPipeline: vi.fn(),
    setBindGroup: vi.fn(),
    setVertexBuffer: vi.fn(),
    setIndexBuffer: vi.fn(),
    draw: vi.fn(),
    drawIndexed: vi.fn(),
    end: vi.fn(),
  };
  const copyTextureToBuffer = vi.fn();
  const device = {
    limits: { maxTextureDimension2D: 8192 },
    queue: { writeBuffer: vi.fn(), submit: vi.fn() },
    createTexture: vi.fn(() => ({ createView: () => ({}), destroy: vi.fn() })),
    createBuffer: vi.fn((descriptor: { size: number }) => ({
      mapAsync: vi.fn(async () => undefined),
      getMappedRange: () => new ArrayBuffer(descriptor.size),
      unmap: vi.fn(),
      destroy: vi.fn(),
    })),
    createCommandEncoder: vi.fn(() => ({
      beginRenderPass: () => pass,
      copyTextureToBuffer,
      finish: () => ({}),
    })),
  };
  const gpu = {
    device,
    exportPipeline: { kind: "images" },
    exportStrokePipeline: { kind: "strokes" },
    exportViewportBuffer: { kind: "uniform" },
    exportViewportBindGroup: { kind: "bind-group" },
    viewportBuffer: { destroy: vi.fn() },
  };
  return { gpu, pass, copyTextureToBuffer };
}

const quad = (width: number, height: number): PinBoardItem["points"] => [
  [0, 0],
  [width, 0],
  [width, height],
  [0, height],
];

function bitmap(itemId: number, width: number, height: number): PinBoardItem {
  return {
    boardId: 1,
    itemId,
    kind: "bitmap",
    width,
    height,
    order: itemId,
    layer: 1,
    deleted: false,
    points: quad(width, height),
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
  };
}

function loadedTexture(maxDimension: number) {
  return {
    texture: { destroy: vi.fn() },
    vertexBuffer: { destroy: vi.fn() },
    outlineBuffer: { destroy: vi.fn() },
    bindGroup: { kind: "texture" },
    byteSize: 64,
    lastUsed: 1,
    maxDimension,
    uvScale: [1, 1] as [number, number],
  };
}

/** 一块小画板的导出替身：只有一张 4×2 的位图，纹理已在缓存里（不会触发 IPC 读取）。 */
function smallBoardRenderer(fake: { gpu: unknown }) {
  return rendererWith({
    loaded: true,
    destroyed: false,
    boardId: 7,
    images: [bitmap(1, 4, 2)],
    textures: new Map([[1, loadedTexture(4)]]),
    gpu: fake.gpu,
    finishActiveInteraction: vi.fn(),
    cancelAutosave: vi.fn(),
    emitState: vi.fn(),
    updateEraserRing: vi.fn(),
    refreshViewport: vi.fn(),
    scheduleAutosave: vi.fn(),
    dirty: false,
  });
}

describe("pin-board canvas export planning", () => {
  it("plans from the visible content bounds and the margin", () => {
    const renderer = rendererWith({
      images: [
        bitmap(1, 100, 80),
        { ...bitmap(2, 400, 400), deleted: true },
        { ...bitmap(3, 400, 400), available: false },
      ],
      gpu: { device: { limits: { maxTextureDimension2D: 8192 } } },
    });

    // 删除的与 DDS 缺失的条目都不参与：它们画不出来，也不该把画布范围撑大。
    const plan = renderer.canvasExportPlan(2, 10, 20_000);
    expect(plan).not.toBeNull();
    expect(plan!.bounds).toEqual({ minX: -10, minY: -10, maxX: 110, maxY: 90 });
    expect(plan!.width).toBe(240);
    expect(plan!.height).toBe(200);
    expect(plan!.scale).toBe(2);
    expect(plan!.overLimit).toBe(false);
  });

  it("returns no plan when every item is hidden or deleted", () => {
    const renderer = rendererWith({
      images: [{ ...bitmap(1, 100, 80), deleted: true }],
      gpu: { device: { limits: { maxTextureDimension2D: 8192 } } },
    });
    expect(renderer.canvasExportPlan(1, 0, 20_000)).toBeNull();
  });

  it("excludes items on hidden layers", () => {
    const renderer = rendererWith({
      images: [bitmap(1, 10, 10), { ...bitmap(2, 500, 500), layer: 3 }],
      visibleLayers: [true, true, true, false],
      gpu: { device: { limits: { maxTextureDimension2D: 8192 } } },
    });
    const plan = renderer.canvasExportPlan(1, 0, 20_000);
    expect(plan!.width).toBe(10);
    expect(plan!.height).toBe(10);
  });

  it("caps the tile width at the device limit", () => {
    const renderer = rendererWith({
      images: [bitmap(1, 9_000, 10)],
      gpu: { device: { limits: { maxTextureDimension2D: 4_096 } } },
    });
    const plan = renderer.canvasExportPlan(1, 0, 20_000);
    expect(plan!.tileWidth).toBe(4_096);
    expect(plan!.tilesPerBand).toBe(3);
  });
});

describe("pin-board canvas export rendering", () => {
  it("renders every tile and hands the raw bytes to the native side", async () => {
    const begin = vi.spyOn(pinBoardApi, "beginPinBoardCanvasExport").mockResolvedValue(undefined);
    const write = vi
      .spyOn(pinBoardApi, "writePinBoardCanvasExportTile")
      .mockResolvedValue(undefined);
    const finish = vi
      .spyOn(pinBoardApi, "finishPinBoardCanvasExport")
      .mockResolvedValue("D:/out/画板.png");
    const cancel = vi
      .spyOn(pinBoardApi, "cancelPinBoardCanvasExport")
      .mockResolvedValue(undefined);
    const fake = exportGpu();
    const onProgress = vi.fn();
    const renderer = smallBoardRenderer(fake);

    const plan = renderer.canvasExportPlan(1, 0, 20_000)!;
    expect(plan.totalTiles).toBe(1);
    const path = await renderer.exportCanvas(plan, {
      directory: "D:/out",
      fileName: "画板",
      onProgress,
      isCancelled: () => false,
    });

    expect(path).toBe("D:/out/画板.png");
    expect(begin).toHaveBeenCalledWith("D:/out", "画板", 4, 2);
    expect(write).toHaveBeenCalledTimes(1);
    const [tile, rgba] = write.mock.calls[0];
    expect(tile).toEqual({ x: 0, y: 0, width: 4, height: 2 });
    // 紧排的 RGBA：4 × 2 × 4 字节，行尾 padding 已经去掉。
    expect(rgba.byteLength).toBe(4 * 2 * 4);
    expect(onProgress).toHaveBeenCalledWith(1, 1);
    expect(finish).toHaveBeenCalledOnce();
    expect(cancel).not.toHaveBeenCalled();
    // 导出走离屏管线，视口按块设置；不画选中描边。
    expect(fake.pass.setPipeline).toHaveBeenCalledWith(fake.gpu.exportPipeline);
    expect(fake.pass.setViewport).toHaveBeenCalledWith(0, 0, 4, 2, 0, 1);
    expect(fake.copyTextureToBuffer).toHaveBeenCalledOnce();
    // 结束后解锁并回到屏幕渲染路径。
    expect(renderer.interactionState.exporting).toBe(false);
    expect(renderer.interactionState.locked).toBe(false);
  });

  it("cancels the native session when the loop is aborted", async () => {
    vi.spyOn(pinBoardApi, "beginPinBoardCanvasExport").mockResolvedValue(undefined);
    const write = vi
      .spyOn(pinBoardApi, "writePinBoardCanvasExportTile")
      .mockResolvedValue(undefined);
    const finish = vi
      .spyOn(pinBoardApi, "finishPinBoardCanvasExport")
      .mockResolvedValue("unused");
    const cancel = vi
      .spyOn(pinBoardApi, "cancelPinBoardCanvasExport")
      .mockResolvedValue(undefined);
    const renderer = smallBoardRenderer(exportGpu());

    const plan = renderer.canvasExportPlan(1, 0, 20_000)!;
    await expect(renderer.exportCanvas(plan, {
      directory: "D:/out",
      fileName: "画板",
      onProgress: vi.fn(),
      isCancelled: () => true,
    })).rejects.toThrow("导出已取消");

    expect(write).not.toHaveBeenCalled();
    expect(finish).not.toHaveBeenCalled();
    expect(cancel).toHaveBeenCalledOnce();
    expect(renderer.interactionState.exporting).toBe(false);
  });

  it("refuses a second concurrent export", async () => {
    vi.spyOn(pinBoardApi, "beginPinBoardCanvasExport").mockResolvedValue(undefined);
    const renderer = rendererWith({
      loaded: true,
      destroyed: false,
      boardId: 7,
      exporting: true,
      images: [bitmap(1, 4, 2)],
      gpu: exportGpu().gpu,
    });
    const plan = renderer.canvasExportPlan(1, 0, 20_000)!;
    await expect(renderer.exportCanvas(plan, {
      directory: "D:/out",
      fileName: "画板",
      onProgress: vi.fn(),
      isCancelled: () => false,
    })).rejects.toThrow("当前画板无法导出");
  });

  it("ignores tool changes while exporting", () => {
    const renderer = rendererWith({
      loaded: true,
      locked: false,
      exporting: true,
      tool: "select",
      images: [],
      emitState: vi.fn(),
    });

    renderer.setTool("pen");
    expect(renderer.interactionState.tool).toBe("select");
  });
});
