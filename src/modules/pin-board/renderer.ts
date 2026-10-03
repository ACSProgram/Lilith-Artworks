import { errorMessage } from "../../shared/errors";
import { pinBoardApi } from "./api";
import type {
  PinBoardClipboardImage,
  PinBoardEditableImage,
  PinBoardImage,
  PinBoardView,
} from "./types";
import {
  DEFAULT_VIEWPORT,
  arrangeImages,
  fitViewportToBounds,
  imageBounds,
  imageDisplayMaxDimension,
  imageSize,
  imagesBounds,
  imagesCenter,
  intersects,
  pointInQuad,
  quadIntersectsBounds,
  quadCenter,
  rotateQuad,
  scaleQuad,
  screenToWorld,
  translateQuad,
  viewportBounds,
  worldToScreen,
  type Bounds,
  type Point,
  type Quad,
  type Viewport,
} from "./geometry";
import type { PinBoardViewSession } from "./session";
import { shortcutCanHandle } from "./shortcuts";
import {
  estimatedTextureLoadBytes,
  estimatedTextureResidentBytes,
  PIN_BOARD_CANVAS_DIMENSION_LIMIT,
  PIN_BOARD_MIN_PREVIEW_DIMENSION,
  PIN_BOARD_NATIVE_TEXTURE_LIMIT,
  PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES,
  PIN_BOARD_TEXTURE_LOAD_BUDGET_BYTES,
  PIN_BOARD_TEXTURE_STAGING_MARGIN_BYTES,
  PIN_BOARD_TEXTURE_UPLOAD_BUDGET_BYTES,
  bc7TextureDimensions,
  bc7TextureUvScale,
  requestedTextureDimension,
  textureBudgetsForLevel,
  textureUploadLayout,
  type PinBoardTextureBudgets,
} from "./texturePolicy";

type GpuState = {
  device: any;
  context: any;
  pipeline: any;
  outlinePipeline: any;
  sampler: any;
  viewportBuffer: any;
  viewportBindGroup: any;
};

type LoadedTexture = {
  texture: any;
  bindGroup: any;
  vertexBuffer: any;
  outlineBuffer: any;
  byteSize: number;
  maxDimension: number;
  uvScale: [number, number];
  lastUsed: number;
};

type TextureLoadCandidate = {
  image: PinBoardImage;
  maxDimension: number;
  estimatedBytes: number;
  priority: number;
  /** 常驻缩略图候选（写入 previewTextures，不参与常规预算淘汰）。 */
  isPreview?: boolean;
};

type ImageSnapshot = {
  imageId: number;
  deleted: boolean;
  layer: number;
  points: Quad;
  uv: Quad;
};

type HistoryEntry = {
  before: ImageSnapshot[];
  after: ImageSnapshot[];
  mergeKey?: string;
  timestamp: number;
};

export interface PinBoardInteractionState {
  imageCount: number;
  selectedCount: number;
  canUndo: boolean;
  canRedo: boolean;
  dirty: boolean;
  saving: boolean;
  locked: boolean;
  zoomPercent: number;
}

export interface PinBoardContextMenuRequest {
  clientX: number;
  clientY: number;
  selectedCount: number;
  targetImage: boolean;
  canEditBoard: boolean;
  canEditSelection: boolean;
}

const PREVIEW_TEXTURE_DIMENSION = PIN_BOARD_MIN_PREVIEW_DIMENSION;
const TEXTURE_LOAD_MEMORY_BUDGET_BYTES = PIN_BOARD_TEXTURE_LOAD_BUDGET_BYTES;
const TEXTURE_LOAD_CONCURRENCY = 2;
const VIEWPORT_LOAD_DELAY_MS = 90;
const MIN_VIEWPORT_HEIGHT = 20;
const MAX_VIEWPORT_HEIGHT = 10_000_000;
/** 画布小于该 CSS 尺寸时不做打开适配，等布局稳定后由 `resize` 补做。 */
const VIEWPORT_FIT_MIN_CANVAS_CSS_PIXELS = 32;
const HISTORY_LIMIT = 100;
const HISTORY_MERGE_MS = 350;
/** 模型变化后静默该时长即自动保存；期间再次变化会重新计时。 */
const AUTOSAVE_DELAY_MS = 1500;
const TRANSFORM_HANDLE_RADIUS = 12;
const ROTATION_HANDLE_OFFSET = 28;
const QUAD_TRIANGLE_ORDER = [0, 1, 3, 3, 1, 2] as const;

const SHADER = `
struct Viewport { bounds: vec4f };
@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(1) @binding(0) var boardSampler: sampler;
@group(1) @binding(1) var boardTexture: texture_2d<f32>;
struct VertexInput { @location(0) position: vec2f, @location(1) uv: vec2f };
struct VertexOutput { @builtin(position) position: vec4f, @location(0) uv: vec2f };
fn projected(position: vec2f) -> vec4f {
  let minPoint = viewport.bounds.xy;
  let extent = max(viewport.bounds.zw, vec2f(1.0));
  let normalized = (position - minPoint) / extent;
  return vec4f(normalized.x * 2.0 - 1.0, normalized.y * 2.0 - 1.0, 0.0, 1.0);
}
@vertex fn vs(input: VertexInput) -> VertexOutput {
  var output: VertexOutput;
  output.position = projected(input.position);
  output.uv = input.uv;
  return output;
}
@fragment fn fs(input: VertexOutput) -> @location(0) vec4f {
  return textureSample(boardTexture, boardSampler, input.uv);
}
@vertex fn outlineVs(@location(0) position: vec2f) -> @builtin(position) vec4f {
  return projected(position);
}
@fragment fn outlineFs() -> @location(0) vec4f {
  return vec4f(1.0, 0.76, 0.12, 1.0);
}
`;

function cloneQuad(quad: Quad): Quad {
  return quad.map((point) => [...point] as Point) as Quad;
}

function cloneImage(image: PinBoardImage): PinBoardImage {
  return { ...image, points: cloneQuad(image.points), uv: cloneQuad(image.uv) };
}

function isRgbaPayload(bytes: Uint8Array): boolean {
  return bytes.byteLength >= 12
    && bytes[0] === 0x52
    && bytes[1] === 0x47
    && bytes[2] === 0x42
    && bytes[3] === 0x41;
}

function ddsHeader(bytes: Uint8Array) {
  if (bytes.byteLength < 148
    || bytes[0] !== 0x44
    || bytes[1] !== 0x44
    || bytes[2] !== 0x53
    || bytes[3] !== 0x20) {
    throw new Error("不是有效的 DDS 文件");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return {
    width: view.getUint32(16, true),
    height: view.getUint32(12, true),
    data: bytes.subarray(148),
  };
}

function checkedPayloadBytes(width: number, height: number, bytesPerPixel: number): number {
  const total = width * height * bytesPerPixel;
  if (!Number.isSafeInteger(total) || total < 1) throw new Error("纹理负载尺寸过大");
  return total;
}

function vertexData(
  image: Pick<PinBoardImage, "points" | "uv">,
  uvScale: readonly [number, number] = [1, 1],
): Float32Array {
  return new Float32Array(QUAD_TRIANGLE_ORDER.flatMap((index) => [
    image.points[index][0],
    image.points[index][1],
    image.uv[index][0] * uvScale[0],
    image.uv[index][1] * uvScale[1],
  ]));
}

function outlineData(image: Pick<PinBoardImage, "points">): Float32Array {
  return new Float32Array([0, 1, 2, 3, 0].flatMap((index) => image.points[index]));
}

function createGeometryBuffers(
  device: any,
  image: PinBoardImage,
  uvScale: readonly [number, number] = [1, 1],
) {
  const vertices = vertexData(image, uvScale);
  let vertexBuffer: any = null;
  let outlineBuffer: any = null;
  try {
    vertexBuffer = device.createBuffer({
      size: vertices.byteLength,
      usage: 0x20 | 0x08,
      mappedAtCreation: true,
    });
    new Float32Array(vertexBuffer.getMappedRange()).set(vertices);
    vertexBuffer.unmap();

    const outline = outlineData(image);
    outlineBuffer = device.createBuffer({
      size: outline.byteLength,
      usage: 0x20 | 0x08,
      mappedAtCreation: true,
    });
    new Float32Array(outlineBuffer.getMappedRange()).set(outline);
    outlineBuffer.unmap();
    return { vertexBuffer, outlineBuffer };
  } catch (error) {
    outlineBuffer?.destroy();
    vertexBuffer?.destroy();
    throw error;
  }
}

async function makeGpu(canvas: HTMLCanvasElement): Promise<GpuState> {
  const browserGpu = (navigator as any).gpu;
  if (!browserGpu) throw new Error("当前 WebView 不支持 WebGPU");
  const adapter = await browserGpu.requestAdapter();
  if (!adapter) throw new Error("无法初始化 WebGPU 适配器");
  if (!adapter.features.has("texture-compression-bc")) {
    throw new Error("当前显卡不支持 BC7 纹理");
  }

  const device = await adapter.requestDevice({ requiredFeatures: ["texture-compression-bc"] });
  const context = (canvas as any).getContext("webgpu");
  const format = browserGpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: "opaque" });
  const module = device.createShaderModule({ code: SHADER });
  const viewportLayout = device.createBindGroupLayout({
    entries: [{
      binding: 0,
      visibility: 0x1,
      buffer: { type: "uniform" },
    }],
  });
  const textureLayout = device.createBindGroupLayout({
    entries: [
      { binding: 0, visibility: 0x2, sampler: { type: "filtering" } },
      { binding: 1, visibility: 0x2, texture: { sampleType: "float" } },
    ],
  });
  const pipeline = device.createRenderPipeline({
    layout: device.createPipelineLayout({ bindGroupLayouts: [viewportLayout, textureLayout] }),
    vertex: {
      module,
      entryPoint: "vs",
      buffers: [{
        arrayStride: 16,
        attributes: [
          { shaderLocation: 0, offset: 0, format: "float32x2" },
          { shaderLocation: 1, offset: 8, format: "float32x2" },
        ],
      }],
    },
    fragment: {
      module,
      entryPoint: "fs",
      targets: [{
        format,
        blend: {
          color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
          alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
        },
      }],
    },
    primitive: { topology: "triangle-list" },
  });
  const outlinePipeline = device.createRenderPipeline({
    layout: device.createPipelineLayout({ bindGroupLayouts: [viewportLayout] }),
    vertex: {
      module,
      entryPoint: "outlineVs",
      buffers: [{
        arrayStride: 8,
        attributes: [{ shaderLocation: 0, offset: 0, format: "float32x2" }],
      }],
    },
    fragment: { module, entryPoint: "outlineFs", targets: [{ format }] },
    primitive: { topology: "line-strip" },
  });
  const sampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
  const viewportBuffer = device.createBuffer({ size: 16, usage: 0x40 | 0x08 });
  const viewportBindGroup = device.createBindGroup({
    layout: viewportLayout,
    entries: [{ binding: 0, resource: { buffer: viewportBuffer } }],
  });
  return {
    device,
    context,
    pipeline,
    outlinePipeline,
    sampler,
    viewportBuffer,
    viewportBindGroup,
  };
}

async function loadTexture(
  gpu: GpuState,
  image: PinBoardImage,
  maxDimension: number,
): Promise<LoadedTexture> {
  const response = await pinBoardApi.readPinBoardTexture(image.boardId, image.imageId, maxDimension);
  const bytes = response instanceof Uint8Array ? response : new Uint8Array(response);
  let width: number;
  let height: number;
  let data: Uint8Array;
  let format: string;
  let sourceBytesPerRow: number;
  let rowsPerImage: number;
  let textureWidth: number;
  let textureHeight: number;
  let uvScale: [number, number];

  if (isRgbaPayload(bytes)) {
    if (bytes.byteLength < 12) throw new Error("RGBA 纹理负载不完整");
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    width = view.getUint32(4, true);
    height = view.getUint32(8, true);
    data = bytes.subarray(12);
    format = "rgba8unorm";
    textureWidth = width;
    textureHeight = height;
    uvScale = [1, 1];
    const expectedBytes = checkedPayloadBytes(width, height, 4);
    if (expectedBytes > PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES || data.byteLength < expectedBytes) {
      throw new Error("RGBA 纹理预览超过内存上限");
    }
    sourceBytesPerRow = width * 4;
    rowsPerImage = height;
  } else {
    const header = ddsHeader(bytes);
    width = header.width;
    height = header.height;
    data = header.data;
    format = "bc7-rgba-unorm";
    sourceBytesPerRow = Math.ceil(width / 4) * 16;
    rowsPerImage = Math.ceil(height / 4);
    [textureWidth, textureHeight] = bc7TextureDimensions(width, height);
    uvScale = bc7TextureUvScale(width, height);
  }

  if (width < 1 || height < 1) {
    throw new Error("纹理负载尺寸无效");
  }
  if (width > PIN_BOARD_NATIVE_TEXTURE_LIMIT || height > PIN_BOARD_NATIVE_TEXTURE_LIMIT) {
    throw new Error("纹理负载超过素材板尺寸上限");
  }
  const upload = textureUploadLayout(data, sourceBytesPerRow, rowsPerImage);
  if (upload.data.byteLength > PIN_BOARD_TEXTURE_UPLOAD_BUDGET_BYTES) {
    throw new Error("纹理上传负载超过内存上限");
  }

  let texture: any = null;
  let geometry: { vertexBuffer: any; outlineBuffer: any } | null = null;
  let validationScope = false;
  try {
    if (typeof gpu.device.pushErrorScope === "function"
      && typeof gpu.device.popErrorScope === "function") {
      gpu.device.pushErrorScope("validation");
      validationScope = true;
    }
    texture = gpu.device.createTexture({
      size: { width: textureWidth, height: textureHeight, depthOrArrayLayers: 1 },
      format,
      usage: 0x04 | 0x02,
    });
    gpu.device.queue.writeTexture(
      { texture },
      upload.data,
      { bytesPerRow: upload.bytesPerRow, rowsPerImage: upload.rowsPerImage },
      { width: textureWidth, height: textureHeight, depthOrArrayLayers: 1 },
    );
    const bindGroup = gpu.device.createBindGroup({
      layout: gpu.pipeline.getBindGroupLayout(1),
      entries: [
        { binding: 0, resource: gpu.sampler },
        { binding: 1, resource: texture.createView() },
      ],
    });
    geometry = createGeometryBuffers(gpu.device, image, uvScale);
    if (validationScope) {
      const validationError = await gpu.device.popErrorScope();
      validationScope = false;
      if (validationError) throw new Error(validationError.message || "WebGPU 纹理校验失败");
    }
    return {
      texture,
      bindGroup,
      ...geometry,
      byteSize: upload.data.byteLength,
      maxDimension: Math.max(width, height),
      uvScale,
      lastUsed: performance.now(),
    };
  } catch (error) {
    if (validationScope) {
      try {
        await gpu.device.popErrorScope();
      } catch {
        // Preserve the original texture error when an error scope cannot close.
      }
    }
    geometry?.outlineBuffer.destroy();
    geometry?.vertexBuffer.destroy();
    texture?.destroy();
    throw error;
  }
}

function destroyTexture(loaded: LoadedTexture) {
  loaded.texture.destroy();
  loaded.vertexBuffer.destroy();
  loaded.outlineBuffer.destroy();
}

function clearColor(canvas: HTMLCanvasElement) {
  const values = getComputedStyle(canvas).backgroundColor.match(/[\d.]+/g)?.map(Number);
  if (!values || values.length < 3) return { r: 0.08, g: 0.1, b: 0.09, a: 1 };
  return {
    r: values[0] / 255,
    g: values[1] / 255,
    b: values[2] / 255,
    a: values.length > 3 ? values[3] : 1,
  };
}

export class PinBoardRenderer {
  private readonly boardId: number;
  private readonly viewport: Viewport = { ...DEFAULT_VIEWPORT };
  private readonly images: PinBoardImage[];
  private readonly imageById = new Map<number, PinBoardImage>();
  private readonly textures = new Map<number, LoadedTexture>();
  /** 常驻缩略图缓存：每个访问过的图片保留一张低分辨率预览，滑动视图时
   *  高清纹理被淘汰后仍可立即绘制，避免闪烁。只在独立预览预算超限时
   *  按「不可见优先 + LRU」淘汰，模块销毁时整体释放。 */
  private readonly previewTextures = new Map<number, LoadedTexture>();
  /** 按目标维度记录加载失败的图片，缩略图（256）失败不会阻止高清重试，
   *  高清失败也不会阻止缩略图兜底。 */
  private readonly failedTextureDimensions = new Map<number, number>();
  /** 高清纹理 LRU 预算（随缓存等级配置变化，见 `setTextureCacheBudgets`）。 */
  private textureBudgetBytes: number;
  /** 常驻缩略图预算（随缓存等级配置变化）。 */
  private previewTextureBudgetBytes: number;
  private readonly textureQualityCaps = new Map<number, number>();
  private readonly selectedIds = new Set<number>();
  private readonly resizeObserver: ResizeObserver;
  private readonly themeObserver: MutationObserver;
  private readonly history: HistoryEntry[] = [];
  private visibleImages: PinBoardImage[] = [];
  private historyIndex = 0;
  private savedStateKey: string;
  private revision: string;
  private cssHeight = 0;
  private defaultWorldUnitsPerCssPixel = 1;
  private loadTimer: number | null = null;
  private loadRequested = false;
  private loading = false;
  private textureReservedBytes = 0;
  private generation = 0;
  private preservedWorldUnitsPerCssPixel: number | null = null;
  /** 打开画板时按图片最小包围框定位视图，等画布尺寸可用后再执行。 */
  private pendingViewportFit = false;
  private destroyed = false;
  private saving = false;
  private finalized = false;
  private active: boolean;
  private dirty = false;
  private autosaveTimer: number | null = null;
  private savePromise: Promise<boolean> | null = null;
  private finalizePromise: Promise<boolean> | null = null;
  private locked = false;
  private dragMode: "pan" | "images" | "marquee" | "rotate" | "scale" | null = null;
  private dragPointerId: number | null = null;
  private pointerX = 0;
  private pointerY = 0;
  private cursorWorld: Point | null = null;
  private dragWorld: Point = [0, 0];
  private marqueeWorld: Point = [0, 0];
  private readonly marqueeBaseIds = new Set<number>();
  private rotateCenter: Point = [0, 0];
  private rotateStartAngle = 0;
  private scaleAnchor: Point = [0, 0];
  private scaleStartDistance = 1;
  private dragBefore: ImageSnapshot[] = [];
  private selectionScreenQuad: Quad | null = null;
  private drawErrorReported = false;

  private constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly selectionElement: HTMLElement,
    private readonly marqueeElement: HTMLElement,
    view: PinBoardView,
    private readonly gpu: GpuState,
    initialSession: PinBoardViewSession | null,
    initiallyActive: boolean,
    private arrangementGapCssPixels: number,
    private autosaveEnabled: boolean,
    private readonly onError: (message: string) => void,
    private readonly onState: (state: PinBoardInteractionState) => void,
    private readonly onSessionChange: (session: PinBoardViewSession) => void,
    private readonly onContextMenu: (request: PinBoardContextMenuRequest | null) => void,
    cacheBudgets: PinBoardTextureBudgets,
  ) {
    this.boardId = view.boardId;
    this.active = initiallyActive;
    this.textureBudgetBytes = cacheBudgets.residentBytes;
    this.previewTextureBudgetBytes = cacheBudgets.previewBytes;
    this.images = view.images.map(cloneImage);
    for (const image of this.images) this.imageById.set(image.imageId, image);
    this.revision = view.revision;
    const cssHeight = Math.max(canvas.getBoundingClientRect().height, 1);
    this.cssHeight = cssHeight;
    this.defaultWorldUnitsPerCssPixel = DEFAULT_VIEWPORT.height / cssHeight;
    if (initialSession) {
      this.viewport.centerX = initialSession.centerX;
      this.viewport.centerY = initialSession.centerY;
      this.viewport.height = this.clampViewportHeight(
        initialSession.worldUnitsPerCssPixel * cssHeight,
      );
      this.locked = initialSession.locked;
      if (!this.locked) {
        for (const imageId of initialSession.selectedImageIds) {
          if (this.imageById.has(imageId)) this.selectedIds.add(imageId);
        }
      }
    } else {
      // 进程内没有该画板的会话：打开时按画板内全部图片的最小包围框定位视图。
      this.locked = true;
      this.pendingViewportFit = true;
    }
    this.savedStateKey = this.stateKey();
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.themeObserver = new MutationObserver(() => this.draw());
    this.resizeObserver.observe(canvas);
    this.themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "data-system-theme"],
    });
    window.addEventListener("resize", this.resize);
    window.addEventListener("keydown", this.keyDown);
    window.addEventListener("pagehide", this.autoSave);
    document.addEventListener("visibilitychange", this.visibilityChange);
    canvas.addEventListener("pointerdown", this.pointerDown);
    canvas.addEventListener("pointermove", this.pointerMove);
    canvas.addEventListener("pointerup", this.pointerUp);
    canvas.addEventListener("pointercancel", this.pointerUp);
    canvas.addEventListener("wheel", this.wheel, { passive: false });
    canvas.addEventListener("contextmenu", this.contextMenu);
    this.resize();
    this.refreshViewport(0);
    this.emitState();

    void gpu.device.lost.then((info: { message?: string }) => {
      if (!this.destroyed) this.onError(`WebGPU 设备已丢失：${info.message || "未知原因"}`);
    });
  }

  static async create(
    canvas: HTMLCanvasElement,
    selectionElement: HTMLElement,
    marqueeElement: HTMLElement,
    view: PinBoardView,
    initialSession: PinBoardViewSession | null,
    initiallyActive: boolean,
    arrangementGapCssPixels: number,
    autosaveEnabled: boolean,
    onError: (message: string) => void,
    onState: (state: PinBoardInteractionState) => void,
    onSessionChange: (session: PinBoardViewSession) => void,
    onContextMenu: (request: PinBoardContextMenuRequest | null) => void,
    cacheBudgets: PinBoardTextureBudgets = textureBudgetsForLevel(undefined),
  ): Promise<PinBoardRenderer> {
    const gpu = await makeGpu(canvas);
    return new PinBoardRenderer(
      canvas,
      selectionElement,
      marqueeElement,
      view,
      gpu,
      initialSession,
      initiallyActive,
      arrangementGapCssPixels,
      autosaveEnabled,
      onError,
      onState,
      onSessionChange,
      onContextMenu,
      cacheBudgets,
    );
  }

  destroy(finalize = true) {
    if (this.destroyed) return;
    // finalize 内部会先保存当前状态，挂起的防抖保存没有必要再触发。
    this.cancelAutosave();
    if (finalize && !this.finalized) void this.finalize();
    this.onSessionChange({
      centerX: this.viewport.centerX,
      centerY: this.viewport.centerY,
      worldUnitsPerCssPixel: this.worldUnitsPerCssPixel(),
      locked: this.locked,
      selectedImageIds: this.locked ? [] : this.selectedImageIds(),
    });
    this.destroyed = true;
    this.generation += 1;
    if (this.loadTimer !== null) window.clearTimeout(this.loadTimer);
    this.resizeObserver.disconnect();
    this.themeObserver.disconnect();
    window.removeEventListener("resize", this.resize);
    window.removeEventListener("keydown", this.keyDown);
    window.removeEventListener("pagehide", this.autoSave);
    document.removeEventListener("visibilitychange", this.visibilityChange);
    this.canvas.removeEventListener("pointerdown", this.pointerDown);
    this.canvas.removeEventListener("pointermove", this.pointerMove);
    this.canvas.removeEventListener("pointerup", this.pointerUp);
    this.canvas.removeEventListener("pointercancel", this.pointerUp);
    this.canvas.removeEventListener("wheel", this.wheel);
    this.canvas.removeEventListener("contextmenu", this.contextMenu);
    for (const loaded of this.textures.values()) destroyTexture(loaded);
    this.textures.clear();
    for (const loaded of this.previewTextures.values()) destroyTexture(loaded);
    this.previewTextures.clear();
    this.textureReservedBytes = 0;
    this.gpu.viewportBuffer.destroy();
  }

  setActive(active: boolean) {
    if (this.active === active) return;
    this.active = active;
    if (!active) {
      this.finishActiveInteraction();
      this.onContextMenu(null);
      this.generation += 1;
      this.loadRequested = false;
      if (this.loadTimer !== null) {
        window.clearTimeout(this.loadTimer);
        this.loadTimer = null;
      }
    } else {
      this.failedTextureDimensions.clear();
      this.refreshViewport(0);
    }
  }

  setArrangementGapCssPixels(gap: number) {
    this.arrangementGapCssPixels = gap;
  }

  /** 配置的缓存等级变化时更新 GPU 缓存预算；下次淘汰/预留立即按新预算执行。 */
  setTextureCacheBudgets(budgets: PinBoardTextureBudgets) {
    this.textureBudgetBytes = budgets.residentBytes;
    this.previewTextureBudgetBytes = budgets.previewBytes;
    this.evictTextures();
  }

  finishActiveInteraction() {
    if (this.dragMode) this.finishInteraction();
  }

  beginViewportResize() {
    this.preservedWorldUnitsPerCssPixel = this.worldUnitsPerCssPixel();
  }

  endViewportResize() {
    this.resize();
    this.preservedWorldUnitsPerCssPixel = null;
  }

  get interactionState(): PinBoardInteractionState {
    return {
      imageCount: this.images.filter((image) => !image.deleted).length,
      selectedCount: this.selectedImages().length,
      canUndo: !this.locked && this.historyIndex > 0,
      canRedo: !this.locked && this.historyIndex < this.history.length,
      dirty: this.dirty,
      saving: this.saving,
      locked: this.locked,
      zoomPercent: Math.round((
        this.defaultWorldUnitsPerCssPixel / this.worldUnitsPerCssPixel()
      ) * 100),
    };
  }

  get boardRevision(): string {
    return this.revision;
  }

  get placementPoint(): Point {
    return this.cursorWorld ? [...this.cursorWorld] as Point : [this.viewport.centerX, this.viewport.centerY];
  }

  placementGap(cssPixels: number): number {
    return this.worldUnitsPerCssPixel() * cssPixels;
  }

  copySelection(): PinBoardClipboardImage[] {
    if (this.locked) return [];
    return this.selectedImages()
      .sort((left, right) => left.order - right.order)
      .map((image) => ({
        displayWidth: imageSize(image)[0],
        displayHeight: imageSize(image)[1],
        sourceBoardId: image.boardId,
        sourceImageId: image.imageId,
        width: image.width,
        height: image.height,
        uv: cloneQuad(image.uv),
      }));
  }

  selectedImageIds(): number[] {
    return this.selectedImages().map((image) => image.imageId);
  }

  copyImageToClipboardId(): number | null {
    const selected = this.selectedImages();
    return selected.length === 1 ? selected[0].imageId : null;
  }

  deleteSelected() {
    const selected = this.selectedImages();
    if (this.locked || selected.length === 0) return;
    const imageIds = selected.map((image) => image.imageId);
    const before = this.snapshot(imageIds);
    for (const image of selected) image.deleted = true;
    const after = this.snapshot(imageIds);
    this.pushHistory(before, after);
    this.selectedIds.clear();
    this.afterModelChange(imageIds);
  }

  applyMutationResult(view: PinBoardView, imageIds: number[]) {
    if (view.boardId !== this.boardId) return;
    const added = imageIds.flatMap((imageId) => view.images.filter((image) => image.imageId === imageId));
    if (added.length === 0) return;
    for (const image of view.images) {
      const current = this.imageById.get(image.imageId);
      if (current) {
        current.order = image.order;
        current.layer = image.layer;
        current.deleted = image.deleted;
      }
    }
    const before = added.map((image) => ({
      imageId: image.imageId,
      deleted: true,
      layer: image.layer,
      points: cloneQuad(image.points),
      uv: cloneQuad(image.uv),
    }));
    for (const image of added) {
      const copy = cloneImage(image);
      this.images.push(copy);
      this.imageById.set(copy.imageId, copy);
      this.selectedIds.add(copy.imageId);
    }
    const after = this.snapshot(imageIds);
    this.revision = view.revision;
    this.savedStateKey = this.stateKey();
    this.pushHistory(before, after);
    this.afterModelChange(imageIds);
  }

  toggleLock() {
    if (!this.locked) {
      this.finishInteraction();
      this.selectedIds.clear();
      this.onContextMenu(null);
    }
    this.locked = !this.locked;
    this.refreshSelectionVisuals();
    this.emitState();
  }

  zoomIn() {
    this.zoomAt(1 / 1.2);
  }

  zoomOut() {
    this.zoomAt(1.2);
  }

  /** 重置视图：画板内有图片时回到“恰好包含全部图片”的视图，否则回默认视图。 */
  resetViewport() {
    const rect = this.canvas.getBoundingClientRect();
    this.applyContentFit(Math.max(rect.width, 1), Math.max(rect.height, 1));
    this.refreshViewport(0);
    this.emitState();
  }

  undo() {
    if (this.locked || this.historyIndex === 0) return;
    const entry = this.history[this.historyIndex - 1];
    this.historyIndex -= 1;
    this.restore(entry.before);
    this.afterModelChange(entry.before.map((image) => image.imageId));
  }

  redo() {
    if (this.locked || this.historyIndex >= this.history.length) return;
    const entry = this.history[this.historyIndex];
    this.historyIndex += 1;
    this.restore(entry.after);
    this.afterModelChange(entry.after.map((image) => image.imageId));
  }

  scaleSelected(factor: number) {
    const selected = this.selectedImages();
    if (selected.length === 0) return;
    if (factor < 1 && selected.some((image) => Math.min(...imageSize(image)) * factor < 0.001)) return;
    const center = imagesCenter(selected);
    this.commitMutation((image) => {
      image.points = scaleQuad(image.points, factor, center);
    });
  }

  rotateSelected(degrees: number) {
    const selected = this.selectedImages();
    if (selected.length === 0) return;
    const center = imagesCenter(selected);
    this.commitMutation((image) => {
      image.points = rotateQuad(image.points, degrees * Math.PI / 180, center);
    });
  }

  flipSelected(horizontal: boolean) {
    this.commitMutation((image) => {
      const [uv0, uv1, uv2, uv3] = image.uv.map((uv) => [...uv] as Point) as Quad;
      image.uv = horizontal
        ? [uv1, uv0, uv3, uv2]
        : [uv3, uv2, uv1, uv0];
    });
  }

  setSelectedLayer(layer: 0 | 1 | 2) {
    this.commitMutation((image) => {
      image.layer = layer;
    });
  }

  arrangeSelected() {
    const selected = this.selectedImages();
    if (selected.length < 2 || this.locked) return;
    const center = imagesCenter(selected);
    const gap = this.worldUnitsPerCssPixel() * this.arrangementGapCssPixels;
    const arranged = arrangeImages(selected, center, gap);
    this.commitMutation((image) => {
      const points = arranged.get(image.imageId);
      if (points) image.points = points;
    });
  }

  save(): Promise<boolean> {
    this.cancelAutosave();
    if (this.savePromise) {
      return this.savePromise.then((saved) => (
        saved && this.dirty ? this.save() : saved
      ));
    }
    const stateKey = this.stateKey();
    if (stateKey === this.savedStateKey) return Promise.resolve(true);
    const revision = this.revision;
    const images = this.editableImages();
    this.saving = true;
    this.emitState();
    const task = pinBoardApi.savePinBoard(this.boardId, images, revision)
      .then((result) => {
        this.revision = result.revision;
        this.savedStateKey = stateKey;
        this.dirty = this.stateKey() !== stateKey;
        // 保存期间又有编辑时，恢复自动保存排程。
        if (this.dirty) this.scheduleAutosave();
        return true;
      })
      .catch((error) => {
        this.onError(`画板保存失败：${errorMessage(error)}`);
        return false;
      })
      .finally(() => {
        if (this.savePromise === task) this.savePromise = null;
        this.saving = false;
        this.emitState();
      });
    this.savePromise = task;
    return task;
  }

  finalize(): Promise<boolean> {
    if (this.finalized) return Promise.resolve(true);
    if (this.finalizePromise) return this.finalizePromise;
    const task = this.save()
      .then((saved) => {
        if (!saved) return false;
        return pinBoardApi.finalizePinBoard(this.boardId, this.revision)
          .then((result) => {
            this.revision = result.revision;
            this.finalized = true;
            return true;
          });
      })
      .catch((error) => {
        this.onError(`画板结算失败：${errorMessage(error)}`);
        return false;
      })
      .finally(() => {
        if (this.finalizePromise === task) this.finalizePromise = null;
      });
    this.finalizePromise = task;
    return task;
  }

  private stateKey(): string {
    return JSON.stringify(this.images.map(({ imageId, order, layer, deleted, points, uv }) => ({
      imageId,
      order,
      layer,
      deleted,
      points,
      uv,
    })));
  }

  private editableImages(): PinBoardEditableImage[] {
    return this.images.map(({ imageId, order, layer, deleted, points, uv }) => ({
      imageId,
      order,
      layer,
      deleted,
      points: cloneQuad(points),
      uv: cloneQuad(uv),
    }));
  }

  private emitState() {
    if (this.destroyed) return;
    this.canvas.classList.toggle("locked", this.locked);
    this.updateSelectionOverlay();
    this.onSessionChange({
      centerX: this.viewport.centerX,
      centerY: this.viewport.centerY,
      worldUnitsPerCssPixel: this.worldUnitsPerCssPixel(),
      locked: this.locked,
      selectedImageIds: this.locked ? [] : this.selectedImageIds(),
    });
    this.onState(this.interactionState);
  }

  private readonly autoSave = () => {
    void this.save();
  };

  private readonly visibilityChange = () => {
    if (document.visibilityState === "hidden") void this.save();
  };

  private readonly keyDown = (event: KeyboardEvent) => {
    if (!this.active) return;
    if (!shortcutCanHandle(event)) return;
    if (event.key === "Delete") {
      event.preventDefault();
      this.finishActiveInteraction();
      this.deleteSelected();
      return;
    }
    if (!(event.ctrlKey || event.metaKey)) return;
    const key = event.key.toLowerCase();
    if (key === "s") {
      event.preventDefault();
      this.finishActiveInteraction();
      void this.save();
    } else if (key === "z" && !event.shiftKey) {
      event.preventDefault();
      this.finishActiveInteraction();
      this.undo();
    } else if (key === "y" || (key === "z" && event.shiftKey)) {
      event.preventDefault();
      this.finishActiveInteraction();
      this.redo();
    }
  };

  private readonly resize = () => {
    if (this.destroyed) return;
    const rect = this.canvas.getBoundingClientRect();
    const cssWidth = Math.max(rect.width, 1);
    const cssHeight = Math.max(rect.height, 1);
    if (this.pendingViewportFit
      && cssWidth >= VIEWPORT_FIT_MIN_CANVAS_CSS_PIXELS
      && cssHeight >= VIEWPORT_FIT_MIN_CANVAS_CSS_PIXELS) {
      // 画布尺寸首次可用时执行打开适配；模块隐藏期间创建的 renderer 会在这里补做。
      this.applyContentFit(cssWidth, cssHeight);
    }
    if (this.preservedWorldUnitsPerCssPixel !== null) {
      this.viewport.height = this.clampViewportHeight(
        this.preservedWorldUnitsPerCssPixel * cssHeight,
      );
    } else if (this.cssHeight > 0 && cssHeight !== this.cssHeight) {
      this.viewport.height = this.clampViewportHeight(
        this.viewport.height * cssHeight / this.cssHeight,
      );
    }
    this.cssHeight = cssHeight;
    const deviceLimit = this.gpu.device.limits.maxTextureDimension2D as number;
    // The swap chain is another GPU allocation. Keep it at the same hard
    // ceiling as board textures, especially on high-DPI displays where a
    // modest CSS canvas can otherwise become a 16K backing surface.
    const maxDimension = Math.min(
      PIN_BOARD_CANVAS_DIMENSION_LIMIT,
      PIN_BOARD_NATIVE_TEXTURE_LIMIT,
      Number.isFinite(deviceLimit) ? Math.max(1, deviceLimit) : PIN_BOARD_NATIVE_TEXTURE_LIMIT,
    );
    const scale = Math.min(
      window.devicePixelRatio || 1,
      maxDimension / cssWidth,
      maxDimension / cssHeight,
    );
    const width = Math.max(1, Math.round(cssWidth * scale));
    const height = Math.max(1, Math.round(cssHeight * scale));
    if (this.canvas.width === width && this.canvas.height === height) {
      this.refreshViewport(0);
      this.emitState();
      return;
    }
    this.canvas.width = width;
    this.canvas.height = height;
    this.refreshViewport(0);
    this.emitState();
  };

  private readonly pointerDown = (event: PointerEvent) => {
    if (event.button !== 0 && event.button !== 1) return;
    if (this.dragMode) return;
    this.onContextMenu(null);
    this.canvas.focus({ preventScroll: true });
    this.pointerX = event.clientX;
    this.pointerY = event.clientY;
    const rect = this.canvas.getBoundingClientRect();
    this.dragWorld = screenToWorld(event.clientX, event.clientY, rect, this.viewport);
    this.cursorWorld = this.dragWorld;

    if (event.button === 1 || this.locked) {
      this.dragMode = "pan";
    } else if (this.rotationHandleHit(event.clientX, event.clientY)) {
      const selection = this.selectionWorldQuad();
      if (!selection) return;
      this.dragMode = "rotate";
      this.dragBefore = this.snapshot([...this.selectedIds]);
      this.rotateCenter = quadCenter(selection);
      this.rotateStartAngle = Math.atan2(
        this.dragWorld[1] - this.rotateCenter[1],
        this.dragWorld[0] - this.rotateCenter[0],
      );
    } else {
      const resizeHandle = this.resizeHandleAt(event.clientX, event.clientY);
      if (resizeHandle !== null) {
        const selection = this.selectionWorldQuad();
        if (!selection) return;
        this.dragMode = "scale";
        this.dragBefore = this.snapshot([...this.selectedIds]);
        this.scaleAnchor = selection[(resizeHandle + 2) % selection.length];
        this.scaleStartDistance = Math.max(
          Math.hypot(this.dragWorld[0] - this.scaleAnchor[0], this.dragWorld[1] - this.scaleAnchor[1]),
          1e-6,
        );
      } else {
        const hit = this.hitImage(this.dragWorld);
        if ((event.ctrlKey || event.metaKey) && hit) {
          if (this.selectedIds.has(hit.imageId)) this.selectedIds.delete(hit.imageId);
          else {
            this.selectedIds.add(hit.imageId);
            this.bringToFront([hit.imageId]);
          }
          this.refreshSelectionVisuals();
          event.preventDefault();
          return;
        }

        const selection = this.selectionWorldQuad();
        const insideSelection = selection
          ? pointInQuad(this.dragWorld, selection)
          : false;
        if (hit && this.selectedIds.has(hit.imageId)) {
          this.dragMode = "images";
          this.dragBefore = this.snapshot([...this.selectedIds]);
        } else if (hit) {
          this.selectedIds.clear();
          this.selectedIds.add(hit.imageId);
          this.bringToFront([hit.imageId]);
          this.dragMode = "images";
          this.dragBefore = this.snapshot([hit.imageId]);
          this.refreshSelectionVisuals();
        } else if (insideSelection) {
          this.dragMode = "images";
          this.dragBefore = this.snapshot([...this.selectedIds]);
        } else {
          this.marqueeBaseIds.clear();
          if (event.ctrlKey || event.metaKey) {
            for (const imageId of this.selectedIds) this.marqueeBaseIds.add(imageId);
          } else {
            this.selectedIds.clear();
          }
          this.dragMode = "marquee";
          this.marqueeWorld = [...this.dragWorld];
          this.updateMarqueeOverlay(this.boundsBetween(this.dragWorld, this.marqueeWorld));
          this.refreshSelectionVisuals();
        }
      }
    }

    this.dragPointerId = event.pointerId;
    this.canvas.setPointerCapture(event.pointerId);
    this.canvas.classList.add(
      this.dragMode === "pan"
        ? "dragging"
        : this.dragMode === "rotate"
          ? "rotating-image"
          : this.dragMode === "scale"
            ? "scaling-image"
            : this.dragMode === "images"
              ? "moving-image"
              : "selecting",
    );
    this.emitState();
    event.preventDefault();
  };

  private readonly pointerMove = (event: PointerEvent) => {
    this.cursorWorld = screenToWorld(
      event.clientX,
      event.clientY,
      this.canvas.getBoundingClientRect(),
      this.viewport,
    );
    if (!this.dragMode || this.dragPointerId !== event.pointerId) {
      const resizeHandle = this.resizeHandleAt(event.clientX, event.clientY);
      this.canvas.classList.toggle(
        "over-rotate-handle",
        !this.locked && this.rotationHandleHit(event.clientX, event.clientY),
      );
      this.canvas.classList.toggle(
        "over-resize-nwse",
        resizeHandle === 0 || resizeHandle === 2,
      );
      this.canvas.classList.toggle(
        "over-resize-nesw",
        resizeHandle === 1 || resizeHandle === 3,
      );
      return;
    }
    if (this.dragMode === "pan") {
      const rect = this.canvas.getBoundingClientRect();
      const unitsPerCssPixel = this.viewport.height / Math.max(rect.height, 1);
      this.viewport.centerX -= (event.clientX - this.pointerX) * unitsPerCssPixel;
      this.viewport.centerY += (event.clientY - this.pointerY) * unitsPerCssPixel;
      this.pointerX = event.clientX;
      this.pointerY = event.clientY;
      this.refreshViewport(VIEWPORT_LOAD_DELAY_MS);
      this.emitState();
    } else {
      const world = screenToWorld(
        event.clientX,
        event.clientY,
        this.canvas.getBoundingClientRect(),
        this.viewport,
      );
      if (this.dragMode === "images") {
        const deltaX = world[0] - this.dragWorld[0];
        const deltaY = world[1] - this.dragWorld[1];
        for (const snapshot of this.dragBefore) {
          const image = this.imageById.get(snapshot.imageId);
          if (image) image.points = translateQuad(snapshot.points, deltaX, deltaY);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.imageId));
      } else if (this.dragMode === "rotate") {
        let radians = Math.atan2(
          world[1] - this.rotateCenter[1],
          world[0] - this.rotateCenter[0],
        ) - this.rotateStartAngle;
        if (event.shiftKey) {
          const increment = Math.PI / 12;
          radians = Math.round(radians / increment) * increment;
        }
        for (const snapshot of this.dragBefore) {
          const image = this.imageById.get(snapshot.imageId);
          if (image) image.points = rotateQuad(snapshot.points, radians, this.rotateCenter);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.imageId));
      } else if (this.dragMode === "scale") {
        const distance = Math.hypot(
          world[0] - this.scaleAnchor[0],
          world[1] - this.scaleAnchor[1],
        );
        const minimumScale = this.dragBefore.reduce((minimum, snapshot) => {
          const width = Math.hypot(
            snapshot.points[1][0] - snapshot.points[0][0],
            snapshot.points[1][1] - snapshot.points[0][1],
          );
          const height = Math.hypot(
            snapshot.points[3][0] - snapshot.points[0][0],
            snapshot.points[3][1] - snapshot.points[0][1],
          );
          return Math.max(minimum, 0.001 / Math.max(Math.min(width, height), 1e-9));
        }, 0);
        const factor = Math.max(distance / this.scaleStartDistance, minimumScale);
        for (const snapshot of this.dragBefore) {
          const image = this.imageById.get(snapshot.imageId);
          if (image) image.points = scaleQuad(snapshot.points, factor, this.scaleAnchor);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.imageId));
      } else {
        this.marqueeWorld = world;
        const bounds = this.boundsBetween(this.dragWorld, world);
        this.selectedIds.clear();
        for (const imageId of this.marqueeBaseIds) this.selectedIds.add(imageId);
        for (const image of this.images) {
          if (!image.deleted && image.available && quadIntersectsBounds(image.points, bounds)) {
            this.selectedIds.add(image.imageId);
          }
        }
        this.updateMarqueeOverlay(bounds);
        this.refreshSelectionVisuals();
      }
    }
    event.preventDefault();
  };

  private readonly pointerUp = (event: PointerEvent) => {
    if (!this.dragMode || this.dragPointerId !== event.pointerId) return;
    this.finishInteraction();
    event.preventDefault();
  };

  private finishInteraction() {
    const pointerId = this.dragPointerId;
    if ((this.dragMode === "images" || this.dragMode === "rotate" || this.dragMode === "scale")
      && this.dragBefore.length > 0) {
      const after = this.snapshot(this.dragBefore.map((image) => image.imageId));
      this.pushHistory(this.dragBefore, after);
      this.afterModelChange(after.map((image) => image.imageId));
    } else if (this.dragMode === "pan") {
      this.scheduleLoads(0);
    } else if (this.dragMode === "marquee") {
      this.updateMarqueeOverlay(null);
      this.bringToFront([...this.selectedIds]);
    }
    if (pointerId !== null && this.canvas.hasPointerCapture(pointerId)) {
      this.canvas.releasePointerCapture(pointerId);
    }
    this.dragMode = null;
    this.dragPointerId = null;
    this.dragBefore = [];
    this.marqueeBaseIds.clear();
    this.canvas.classList.remove(
      "dragging",
      "moving-image",
      "rotating-image",
      "scaling-image",
      "selecting",
      "over-rotate-handle",
      "over-resize-nwse",
      "over-resize-nesw",
    );
    this.emitState();
  }

  private readonly wheel = (event: WheelEvent) => {
    if (this.dragMode) {
      event.preventDefault();
      return;
    }
    const point = screenToWorld(
      event.clientX,
      event.clientY,
      this.canvas.getBoundingClientRect(),
      this.viewport,
    );
    this.cursorWorld = point;
    this.zoomAt(event.deltaY < 0 ? 1 / 1.15 : 1.15, event.clientX, event.clientY);
    event.preventDefault();
  };

  private readonly contextMenu = (event: MouseEvent) => {
    event.preventDefault();
    if (this.dragMode) {
      this.onContextMenu(null);
      return;
    }
    const point = screenToWorld(
      event.clientX,
      event.clientY,
      this.canvas.getBoundingClientRect(),
      this.viewport,
    );
    this.cursorWorld = point;
    const hit = this.hitImage(point);
    if (!this.locked && hit && !this.selectedIds.has(hit.imageId)) {
      this.selectedIds.clear();
      this.selectedIds.add(hit.imageId);
      this.bringToFront([hit.imageId]);
      this.refreshSelectionVisuals();
    }
    this.onContextMenu({
      clientX: event.clientX,
      clientY: event.clientY,
      selectedCount: this.selectedImages().length,
      targetImage: Boolean(hit && !this.locked),
      canEditBoard: !this.locked,
      canEditSelection: !this.locked && this.selectedIds.size > 0,
    });
  };

  private boundsBetween(start: Point, end: Point): Bounds {
    return {
      minX: Math.min(start[0], end[0]),
      minY: Math.min(start[1], end[1]),
      maxX: Math.max(start[0], end[0]),
      maxY: Math.max(start[1], end[1]),
    };
  }

  private rotationHandleHit(clientX: number, clientY: number): boolean {
    const handle = this.rotationHandlePoint();
    if (this.locked || !handle) return false;
    const rect = this.canvas.getBoundingClientRect();
    return Math.hypot(handle[0] + rect.left - clientX, handle[1] + rect.top - clientY)
      <= TRANSFORM_HANDLE_RADIUS;
  }

  private resizeHandleAt(clientX: number, clientY: number): number | null {
    if (this.locked || !this.selectionScreenQuad) return null;
    const rect = this.canvas.getBoundingClientRect();
    const x = clientX - rect.left;
    const y = clientY - rect.top;
    const index = this.selectionScreenQuad.findIndex((point) => Math.hypot(point[0] - x, point[1] - y)
      <= TRANSFORM_HANDLE_RADIUS);
    return index >= 0 ? index : null;
  }

  private rotationHandlePoint(): Point | null {
    const quad = this.selectionScreenQuad;
    if (!quad) return null;
    const center = quadCenter(quad);
    const top: Point = [
      (quad[0][0] + quad[1][0]) / 2,
      (quad[0][1] + quad[1][1]) / 2,
    ];
    const direction: Point = [top[0] - center[0], top[1] - center[1]];
    const length = Math.hypot(direction[0], direction[1]);
    if (length < 1e-6) return top;
    return [top[0] + direction[0] / length * ROTATION_HANDLE_OFFSET, top[1] + direction[1] / length * ROTATION_HANDLE_OFFSET];
  }

  private updateSelectionOverlay() {
    const selected = this.selectedImages();
    const bounds = this.locked ? null : imagesBounds(selected);
    if (!bounds || selected.length === 0) {
      this.selectionScreenQuad = null;
      this.selectionElement.hidden = true;
      return;
    }
    const rect = this.canvas.getBoundingClientRect();
    const worldQuad = this.selectionWorldQuad();
    if (!worldQuad) return;
    const screenQuad = worldQuad.map((point) => worldToScreen(point, rect, this.viewport)) as Quad;
    this.selectionScreenQuad = screenQuad;
    this.selectionElement.hidden = false;
    const width = Math.max(Math.hypot(screenQuad[1][0] - screenQuad[0][0], screenQuad[1][1] - screenQuad[0][1]), 1);
    const height = Math.max(Math.hypot(screenQuad[3][0] - screenQuad[0][0], screenQuad[3][1] - screenQuad[0][1]), 1);
    const angle = Math.atan2(screenQuad[1][1] - screenQuad[0][1], screenQuad[1][0] - screenQuad[0][0]);
    this.selectionElement.style.transformOrigin = "0 0";
    this.selectionElement.style.transform = `translate(${screenQuad[0][0]}px, ${screenQuad[0][1]}px) rotate(${angle}rad)`;
    this.selectionElement.style.width = `${width}px`;
    this.selectionElement.style.height = `${height}px`;
  }

  private selectionWorldQuad(): Quad | null {
    const selected = this.selectedImages();
    if (selected.length === 1) return cloneQuad(selected[0].points);
    const bounds = imagesBounds(selected);
    return bounds ? [
      [bounds.minX, bounds.maxY],
      [bounds.maxX, bounds.maxY],
      [bounds.maxX, bounds.minY],
      [bounds.minX, bounds.minY],
    ] : null;
  }

  private updateMarqueeOverlay(bounds: Bounds | null) {
    if (!bounds) {
      this.marqueeElement.hidden = true;
      return;
    }
    const rect = this.canvas.getBoundingClientRect();
    const topLeft = worldToScreen([bounds.minX, bounds.maxY], rect, this.viewport);
    const bottomRight = worldToScreen([bounds.maxX, bounds.minY], rect, this.viewport);
    const left = Math.min(topLeft[0], bottomRight[0]);
    const top = Math.min(topLeft[1], bottomRight[1]);
    this.marqueeElement.hidden = false;
    this.marqueeElement.style.transform = `translate(${left}px, ${top}px)`;
    this.marqueeElement.style.width = `${Math.abs(bottomRight[0] - topLeft[0])}px`;
    this.marqueeElement.style.height = `${Math.abs(bottomRight[1] - topLeft[1])}px`;
  }

  private refreshSelectionVisuals() {
    this.sortVisibleImages();
    this.draw();
    this.emitState();
  }

  private zoomAt(factor: number, clientX?: number, clientY?: number) {
    const rect = this.canvas.getBoundingClientRect();
    const anchorX = clientX ?? rect.left + rect.width / 2;
    const anchorY = clientY ?? rect.top + rect.height / 2;
    const point = screenToWorld(anchorX, anchorY, rect, this.viewport);
    const height = this.clampViewportHeight(this.viewport.height * factor);
    const ratioX = (anchorX - rect.left) / Math.max(rect.width, 1);
    const ratioY = (anchorY - rect.top) / Math.max(rect.height, 1);
    const width = height * Math.max(rect.width, 1) / Math.max(rect.height, 1);
    this.viewport.height = height;
    this.viewport.centerX = point[0] - (ratioX - 0.5) * width;
    this.viewport.centerY = point[1] + (ratioY - 0.5) * height;
    this.refreshViewport(VIEWPORT_LOAD_DELAY_MS);
    this.emitState();
  }

  private worldUnitsPerCssPixel(): number {
    return this.viewport.height / Math.max(this.cssHeight, 1);
  }

  private clampViewportHeight(height: number): number {
    return Math.min(MAX_VIEWPORT_HEIGHT, Math.max(MIN_VIEWPORT_HEIGHT, height));
  }

  /** 画板内全部未删除图片的轴对齐最小包围框（旋转图片按外接矩形计算）。 */
  private contentBounds(): Bounds | null {
    return imagesBounds(this.images.filter((image) => !image.deleted));
  }

  /** 按图片包围框适配视图；画板内没有图片时回落到默认视图。 */
  private applyContentFit(cssWidth: number, cssHeight: number) {
    this.pendingViewportFit = false;
    const bounds = this.contentBounds();
    if (!bounds) {
      this.viewport.centerX = DEFAULT_VIEWPORT.centerX;
      this.viewport.centerY = DEFAULT_VIEWPORT.centerY;
      this.viewport.height = this.clampViewportHeight(
        this.defaultWorldUnitsPerCssPixel * cssHeight,
      );
      return;
    }
    const fitted = fitViewportToBounds(bounds, cssWidth, cssHeight);
    this.viewport.centerX = fitted.centerX;
    this.viewport.centerY = fitted.centerY;
    this.viewport.height = this.clampViewportHeight(fitted.height);
  }

  private selectedImages(): PinBoardImage[] {
    return this.images.filter((image) => !image.deleted && this.selectedIds.has(image.imageId));
  }

  private bringToFront(imageIds: number[]) {
    if (imageIds.length === 0) return;
    const promotedIds = new Set(imageIds);
    const ordered = [...this.images].sort((left, right) => (
      left.order - right.order || left.imageId - right.imageId
    ));
    const next = [
      ...ordered.filter((image) => promotedIds.has(image.imageId)),
      ...ordered.filter((image) => !promotedIds.has(image.imageId)),
    ];
    let changed = false;
    next.forEach((image, order) => {
      changed ||= image.order !== order;
      image.order = order;
    });
    if (changed) {
      this.dirty = this.stateKey() !== this.savedStateKey;
      if (this.dirty) this.scheduleAutosave();
      this.sortVisibleImages();
      this.draw();
    }
  }

  private hitImage(point: Point): PinBoardImage | null {
    for (let index = this.visibleImages.length - 1; index >= 0; index -= 1) {
      const image = this.visibleImages[index];
      if (pointInQuad(point, image.points)) return image;
    }
    return null;
  }

  private snapshot(imageIds: number[]): ImageSnapshot[] {
    return imageIds.flatMap((imageId) => {
      const image = this.imageById.get(imageId);
      return image ? [{
        imageId,
        deleted: image.deleted,
        layer: image.layer,
        points: cloneQuad(image.points),
        uv: cloneQuad(image.uv),
      }] : [];
    });
  }

  private restore(snapshots: ImageSnapshot[]) {
    for (const snapshot of snapshots) {
      const image = this.imageById.get(snapshot.imageId);
      if (!image) continue;
      image.deleted = snapshot.deleted;
      image.layer = snapshot.layer;
      image.points = cloneQuad(snapshot.points);
      image.uv = cloneQuad(snapshot.uv);
    }
  }

  private commitMutation(
    mutate: (image: PinBoardImage) => void,
    mergeKey?: string,
  ) {
    if (this.locked || this.selectedIds.size === 0) return;
    const imageIds = [...this.selectedIds];
    const before = this.snapshot(imageIds);
    for (const imageId of imageIds) {
      const image = this.imageById.get(imageId);
      if (image) mutate(image);
    }
    const after = this.snapshot(imageIds);
    this.pushHistory(before, after, mergeKey);
    this.afterModelChange(imageIds);
  }

  private pushHistory(before: ImageSnapshot[], after: ImageSnapshot[], mergeKey?: string) {
    if (JSON.stringify(before) === JSON.stringify(after)) return;
    const timestamp = performance.now();
    const previous = this.history[this.historyIndex - 1];
    if (mergeKey
      && this.historyIndex === this.history.length
      && previous?.mergeKey === mergeKey
      && timestamp - previous.timestamp <= HISTORY_MERGE_MS) {
      previous.after = after;
      previous.timestamp = timestamp;
      return;
    }
    this.history.splice(this.historyIndex);
    this.history.push({ before, after, mergeKey, timestamp });
    if (this.history.length > HISTORY_LIMIT) this.history.shift();
    this.historyIndex = this.history.length;
  }

  private afterLiveChange(imageIds: number[]) {
    this.dirty = true;
    this.syncGeometryBuffers(imageIds);
    this.refreshViewport(VIEWPORT_LOAD_DELAY_MS);
    this.emitState();
  }

  private afterModelChange(imageIds: number[]) {
    for (const imageId of [...this.selectedIds]) {
      if (this.imageById.get(imageId)?.deleted) this.selectedIds.delete(imageId);
    }
    this.dirty = this.stateKey() !== this.savedStateKey;
    if (this.dirty) this.scheduleAutosave();
    else this.cancelAutosave();
    this.syncGeometryBuffers(imageIds);
    this.refreshViewport(0);
    this.emitState();
  }

  private scheduleAutosave() {
    if (!this.autosaveEnabled || this.destroyed) return;
    if (this.autosaveTimer !== null) window.clearTimeout(this.autosaveTimer);
    this.autosaveTimer = window.setTimeout(() => {
      this.autosaveTimer = null;
      void this.save();
    }, AUTOSAVE_DELAY_MS);
  }

  private cancelAutosave() {
    if (this.autosaveTimer !== null) {
      window.clearTimeout(this.autosaveTimer);
      this.autosaveTimer = null;
    }
  }

  /** 设置页开关：关闭时取消挂起的自动保存，开启时若有未保存编辑立即排程。 */
  setAutosaveEnabled(enabled: boolean) {
    if (this.autosaveEnabled === enabled) return;
    this.autosaveEnabled = enabled;
    if (!enabled) {
      this.cancelAutosave();
    } else if (this.dirty) {
      this.scheduleAutosave();
    }
  }

  private syncGeometryBuffers(imageIds: number[]) {
    for (const imageId of imageIds) {
      const loaded = this.textures.get(imageId) ?? this.previewTextures.get(imageId);
      const image = this.imageById.get(imageId);
      if (!loaded || !image) continue;
      this.gpu.device.queue.writeBuffer(
        loaded.vertexBuffer,
        0,
        vertexData(image, loaded.uvScale ?? [1, 1]),
      );
      this.gpu.device.queue.writeBuffer(loaded.outlineBuffer, 0, outlineData(image));
    }
  }

  private refreshViewport(loadDelay: number) {
    if (this.destroyed || this.canvas.width < 1 || this.canvas.height < 1) return;
    this.generation += 1;
    const bounds = viewportBounds(this.viewport, this.canvas.width, this.canvas.height);
    this.visibleImages = this.images.filter((image) => (
      !image.deleted && image.available && intersects(imageBounds(image), bounds)
    ));
    this.sortVisibleImages();
    const now = performance.now();
    for (const image of this.visibleImages) {
      const loaded = this.textures.get(image.imageId);
      if (loaded) loaded.lastUsed = now;
      const preview = this.previewTextures.get(image.imageId);
      if (preview) preview.lastUsed = now;
    }
    this.gpu.device.queue.writeBuffer(
      this.gpu.viewportBuffer,
      0,
      new Float32Array([
        bounds.minX,
        bounds.minY,
        bounds.maxX - bounds.minX,
        bounds.maxY - bounds.minY,
      ]),
    );
    this.evictTextures();
    this.draw();
    this.scheduleLoads(loadDelay);
  }

  private draw() {
    if (this.destroyed) return;
    try {
      const encoder = this.gpu.device.createCommandEncoder();
      const pass = encoder.beginRenderPass({
        colorAttachments: [{
          view: this.gpu.context.getCurrentTexture().createView(),
          clearValue: clearColor(this.canvas),
          loadOp: "clear",
          storeOp: "store",
        }],
      });
      pass.setPipeline(this.gpu.pipeline);
      pass.setBindGroup(0, this.gpu.viewportBindGroup);
      for (const image of this.visibleImages) {
        const loaded = this.textures.get(image.imageId) ?? this.previewTextures.get(image.imageId);
        if (!loaded) continue;
        pass.setBindGroup(1, loaded.bindGroup);
        pass.setVertexBuffer(0, loaded.vertexBuffer);
        pass.draw(6);
      }
      if (this.selectedIds.size > 0) {
        pass.setPipeline(this.gpu.outlinePipeline);
        pass.setBindGroup(0, this.gpu.viewportBindGroup);
        for (const image of this.visibleImages) {
          if (!this.selectedIds.has(image.imageId)) continue;
          const loaded = this.textures.get(image.imageId) ?? this.previewTextures.get(image.imageId);
          if (!loaded) continue;
          pass.setVertexBuffer(0, loaded.outlineBuffer);
          pass.draw(5);
        }
      }
      pass.end();
      this.gpu.device.queue.submit([encoder.finish()]);
      this.drawErrorReported = false;
    } catch (error) {
      if (!this.destroyed && !this.drawErrorReported) {
        this.drawErrorReported = true;
        this.onError(`画布渲染失败：${errorMessage(error)}`);
      }
    }
  }

  private sortVisibleImages() {
    this.visibleImages.sort((left, right) => (
      left.layer - right.layer
      || right.order - left.order
    ));
  }

  private scheduleLoads(delay: number) {
    if (!this.active || this.destroyed) return;
    if (this.loadTimer !== null) window.clearTimeout(this.loadTimer);
    this.loadTimer = window.setTimeout(() => {
      this.loadTimer = null;
      this.loadRequested = true;
      if (!this.loading) void this.drainLoads();
    }, delay);
  }

  private async drainLoads() {
    this.loading = true;
    try {
      while (this.loadRequested && this.active && !this.destroyed) {
        this.loadRequested = false;
        const generation = this.generation;
        await this.loadCandidates(this.textureLoadCandidates(), generation);
      }
    } finally {
      this.loading = false;
      if (this.loadRequested && this.active && !this.destroyed) void this.drainLoads();
    }
  }

  private textureLoadCandidates(): TextureLoadCandidate[] {
    const candidates = this.visibleImages.flatMap((image) => {
      const center = quadCenter(image.points);
      const [width, height] = imageSize(image);
      const distanceSquared = (center[0] - this.viewport.centerX) ** 2
        + (center[1] - this.viewport.centerY) ** 2;
      const priority = distanceSquared / Math.max(width * height, 1);
      const list: TextureLoadCandidate[] = [];
      // 常驻缩略图兜底：没有缩略图时先生成缩略图候选，保证滑动回来后
      // 即使高清纹理尚未加载也能立即绘制，不闪烁。缩略图失败按维度记录，
      // 不会阻止更高分辨率的重试。
      if (!this.previewTextures.has(image.imageId)
        && !this.failedAtOrAbove(image.imageId, PREVIEW_TEXTURE_DIMENSION)) {
        list.push({
          image,
          maxDimension: PREVIEW_TEXTURE_DIMENSION,
          estimatedBytes: estimatedTextureLoadBytes(image, PREVIEW_TEXTURE_DIMENSION),
          priority,
          isPreview: true,
        });
      }
      const desiredDimension = this.requestedDimension(image);
      const qualityCap = this.textureQualityCaps.get(image.imageId);
      const maxDimension = qualityCap
        ? Math.min(desiredDimension, qualityCap)
        : desiredDimension;
      const current = this.textures.get(image.imageId);
      if (!(current && current.maxDimension >= maxDimension * 0.9)
        && !this.failedAtOrAbove(image.imageId, maxDimension)) {
        list.push({
          image,
          maxDimension,
          estimatedBytes: estimatedTextureLoadBytes(image, maxDimension),
          priority,
        });
      }
      return list;
    });
    return candidates.sort((left, right) => (
      Number(this.selectedIds.has(right.image.imageId))
        - Number(this.selectedIds.has(left.image.imageId))
      // 缩略图优先于高清，先给可见图片铺上兜底内容，再逐张清晰化。
      || Number(right.isPreview === true) - Number(left.isPreview === true)
      || left.priority - right.priority
      || right.maxDimension - left.maxDimension
    ));
  }

  /** 该图片在目标尺寸（及其之上）是否已记录过加载失败。 */
  private failedAtOrAbove(imageId: number, maxDimension: number): boolean {
    const failed = this.failedTextureDimensions.get(imageId);
    return failed !== undefined && failed >= maxDimension * 0.9;
  }

  private loadCandidates(candidates: TextureLoadCandidate[], generation: number): Promise<void> {
    return new Promise((resolve) => {
      let nextIndex = 0;
      let activeCount = 0;
      let activeBytes = 0;
      const launch = () => {
        if (this.destroyed || generation !== this.generation) {
          if (activeCount === 0) resolve();
          return;
        }
        while (activeCount < TEXTURE_LOAD_CONCURRENCY && nextIndex < candidates.length) {
          const candidate = candidates[nextIndex];
          if (activeCount > 0
            && activeBytes + candidate.estimatedBytes > TEXTURE_LOAD_MEMORY_BUDGET_BYTES) break;
          nextIndex += 1;
          activeCount += 1;
          activeBytes += candidate.estimatedBytes;
          void this.loadCandidate(candidate, generation).finally(() => {
            activeCount -= 1;
            activeBytes -= candidate.estimatedBytes;
            if (activeCount === 0 && nextIndex >= candidates.length) resolve();
            else launch();
          });
        }
        if (activeCount === 0 && nextIndex >= candidates.length) resolve();
      };
      launch();
    });
  }

  private async loadCandidate(candidate: TextureLoadCandidate, generation: number) {
    const { image, maxDimension, isPreview } = candidate;
    const target = isPreview ? this.previewTextures : this.textures;
    let current: LoadedTexture | null = target.get(image.imageId) ?? null;
    let loaded: LoadedTexture | null = null;
    let lastError: unknown = null;
    try {
      if (current && current.maxDimension >= maxDimension * 0.9) return;

      // A high-zoom replacement can otherwise coexist with its old preview
      // until the post-load eviction pass, creating a large allocation spike.
      const dimensions = [maxDimension];
      const fallback = Math.max(
        PIN_BOARD_MIN_PREVIEW_DIMENSION,
        Math.floor(maxDimension / 2),
      );
      if (fallback < maxDimension) dimensions.push(fallback);

      for (const dimension of dimensions) {
        const residentBytes = estimatedTextureResidentBytes(image, dimension);
        const reservationBytes = residentBytes + PIN_BOARD_TEXTURE_STAGING_MARGIN_BYTES;
        current = target.get(image.imageId) ?? null;
        const reserved = this.reserveTextureBytes(image.imageId, reservationBytes, isPreview);
        if (!reserved) {
          lastError = new Error("纹理显存预算不足");
          continue;
        }
        try {
          loaded = await loadTexture(this.gpu, image, dimension);
          break;
        } catch (error) {
          lastError = error;
        } finally {
          this.textureReservedBytes = Math.max(0, this.textureReservedBytes - reservationBytes);
        }
      }
      if (!loaded) throw lastError ?? new Error("纹理加载失败");
      if (this.destroyed || generation !== this.generation) {
        destroyTexture(loaded);
        return;
      }
      // 高清纹理还要确认图片仍在可见集；缩略图常驻，滑出视口后继续保留。
      if (!isPreview && !this.visibleImages.some((visible) => visible.imageId === image.imageId)) {
        destroyTexture(loaded);
        return;
      }
      const replacement = target.get(image.imageId);
      if (replacement && replacement.maxDimension >= maxDimension * 0.9) {
        destroyTexture(loaded);
        return;
      }
      if (replacement) destroyTexture(replacement);
      target.set(image.imageId, loaded);
      if (!isPreview) {
        if (loaded.maxDimension < maxDimension) {
          this.textureQualityCaps.set(image.imageId, loaded.maxDimension);
        } else {
          this.textureQualityCaps.delete(image.imageId);
        }
      }
      this.evictTextures(image.imageId, 0, isPreview);
      this.failedTextureDimensions.delete(image.imageId);
      this.draw();
    } catch (error) {
      if (this.destroyed || generation !== this.generation) return;
      const previous = this.failedTextureDimensions.get(image.imageId) ?? 0;
      this.failedTextureDimensions.set(image.imageId, Math.max(previous, maxDimension));
      // 缩略图失败静默（高清失败仍按原样上报），避免预算波动反复打扰用户。
      if (!isPreview) this.onError(`图片 ${image.imageId} 加载失败：${errorMessage(error)}`);
    }
  }

  private requestedDimension(image: PinBoardImage): number {
    const displayDimension = imageDisplayMaxDimension(image, this.viewport, this.canvas.height);
    const deviceLimit = this.gpu.device.limits.maxTextureDimension2D as number;
    return requestedTextureDimension(image, displayDimension, deviceLimit);
  }

  private evictTextures(protectedImageId?: number, additionalBytes = 0, isPreview = false) {
    const target = isPreview ? this.previewTextures : this.textures;
    const budget = isPreview ? this.previewTextureBudgetBytes : this.textureBudgetBytes;
    let total = [...target.values()].reduce((sum, loaded) => sum + loaded.byteSize, 0);
    if (total + additionalBytes <= budget) return;
    const visible = new Set(this.visibleImages.map((image) => image.imageId));
    const candidates = [...target.entries()]
      .filter(([imageId]) => imageId !== protectedImageId)
      .sort((left, right) => {
        const visibilityOrder = Number(visible.has(left[0])) - Number(visible.has(right[0]));
        if (visibilityOrder !== 0) return visibilityOrder;
        // 高清淘汰优先选择已有缩略图兜底的图片，避免淘汰后画面空白闪烁；
        // 缩略图淘汰时本项恒相等（previewTextures 自身就是兜底），退化为 LRU。
        const leftCovered = Number(isPreview || this.previewTextures.has(left[0]));
        const rightCovered = Number(isPreview || this.previewTextures.has(right[0]));
        if (leftCovered !== rightCovered) return rightCovered - leftCovered;
        // 大图（纹理字节多）重新解码成本高、等待久，优先淘汰小图：小图重载
        // 快且有缩略图兜底，大图尽量保留，避免「划走再划回」反复等大图。
        if (left[1].byteSize !== right[1].byteSize) return left[1].byteSize - right[1].byteSize;
        return left[1].lastUsed - right[1].lastUsed;
      });
    for (const [imageId, loaded] of candidates) {
      destroyTexture(loaded);
      target.delete(imageId);
      total -= loaded.byteSize;
      if (total + additionalBytes <= budget) break;
    }
  }

  private reserveTextureBytes(
    protectedImageId: number,
    additionalBytes: number,
    isPreview = false,
  ): boolean {
    const target = isPreview ? this.previewTextures : this.textures;
    const budget = isPreview ? this.previewTextureBudgetBytes : this.textureBudgetBytes;
    this.evictTextures(protectedImageId, additionalBytes + this.textureReservedBytes, isPreview);
    let total = [...target.values()].reduce((sum, loaded) => sum + loaded.byteSize, 0);
    const current = target.get(protectedImageId);
    if (current && total + this.textureReservedBytes + additionalBytes > budget) {
      destroyTexture(current);
      target.delete(protectedImageId);
      total -= current.byteSize;
      this.draw();
    }
    if (total + this.textureReservedBytes + additionalBytes > budget) {
      return false;
    }
    this.textureReservedBytes += additionalBytes;
    return true;
  }
}
