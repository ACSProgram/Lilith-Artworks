import { errorMessage } from "../../shared/errors";
import { diagnosticsLog, diagnosticsSlowStep } from "../../shared/diagnostics";
import { pinBoardApi } from "./api";
import {
  recordBoardRevision,
  resolveBoardRevision,
  stageBoardSettle,
  takeBoardSettle,
} from "./pendingSettle";
import type {
  PinBoardArrangeResult,
  PinBoardClipboardImage,
  PinBoardEditableItem,
  PinBoardItem,
  PinBoardStrokeItem,
  PinBoardTextItem,
  PinBoardView,
  SavePinBoardResult,
} from "./types";
import { parseStrokeBatch, type PinBoardStrokeGeometry } from "./strokeGeometry";
import {
  DEFAULT_STROKE_COLOR,
  DEFAULT_STROKE_WIDTH,
  MIN_STROKE_DRAG_CSS_PIXELS,
  beginStrokeDraft,
  clampStrokeWidth,
  extendStrokeDraft,
  isStrokeTool,
  resolveStrokeDraft,
  strokeDraftPoints,
  toolForShortcut,
  type PinBoardTool,
  type StrokeCommitRequest,
  type StrokeDraft,
} from "./strokeDrawing";
import {
  STROKE_OUTLINE_VERTICES,
  STROKE_VERTEX_BYTES,
  buildStrokeBatch,
  buildStrokeOutlines,
  type StrokeBatch,
} from "./strokeRender";
import {
  DEFAULT_VIEWPORT,
  fitViewportToBounds,
  imageBounds,
  imageDisplayMaxDimension,
  imageSize,
  imagesBounds,
  imagesCenter,
  intersects,
  pointInQuad,
  quadCenter,
  rotateQuad,
  scaleQuad,
  screenToWorld,
  snapRotationDelta,
  translateQuad,
  viewportBounds,
  worldToScreen,
  type Bounds,
  type Point,
  type Quad,
  type Viewport,
} from "./geometry";
import type { PinBoardViewSession } from "./session";
import { isAdditiveSelectionModifier, marqueeSelection, sameSelection } from "./selection";
import {
  DEFAULT_LAYER_VISIBILITY,
  PIN_BOARD_LAYER_COUNT,
  STROKE_LAYER,
  hiddenSelectionIds,
  isLayerVisible,
  normalizeLayerVisibility,
  type PinBoardLayerVisibility,
} from "./layers";
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
import {
  requestedTextTextureDimension,
  textRgbaPayload,
  textTextureResidentBytes,
  textTextureRgbaBytes,
} from "./textLayout";

/** 结算遇到 `repository_busy` 时的重试间隔。 */
const FINALIZE_BUSY_RETRY_DELAY_MS = 300;

/**
 * 结算失败是否是「仓库正在写入」。
 *
 * 素材板命令仍以字符串错误拒绝，因此同时接受结构化 `code` 与两种用户文案
 * （同进程排队超时 / 另一个进程写入）。
 */
function isRepositoryBusy(error: unknown): boolean {
  if ((error as { code?: unknown } | null)?.code === "repository_busy") return true;
  const message = errorMessage(error);
  return message.includes("仓库正在写入") || message.includes("另一个进程写入");
}

type GpuState = {
  canvas: HTMLCanvasElement;
  device: any;
  context: any;
  pipeline: any;
  outlinePipeline: any;
  /** 笔迹 SDF 管线：整板笔迹合并成一条三角形列表一次画完。 */
  strokePipeline: any;
  sampler: any;
  viewportBuffer: any;
  viewportBindGroup: any;
  /** 诊断用：本设备的创建序号，便于在日志里与销毁记录配对。 */
  sequence: number;
  /** 设备释放后，异步纹理读取返回时不得再次触碰 WebGPU 对象。 */
  released: boolean;
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
  image: PinBoardItem;
  maxDimension: number;
  estimatedBytes: number;
  priority: number;
  /** 常驻缩略图候选（写入 previewTextures，不参与常规预算淘汰）。 */
  isPreview?: boolean;
};

type ImageSnapshot = {
  itemId: number;
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
  /** 选中里可参与阵列排序的位图数量（文字条目不参与）。 */
  arrangeableCount: number;
  /** 选中里可导出的条目数量（位图与文字；笔迹不参与导出）。 */
  exportableCount: number;
  canUndo: boolean;
  canRedo: boolean;
  dirty: boolean;
  saving: boolean;
  locked: boolean;
  zoomPercent: number;
  /** 当前工具：`select` 走选中/变换，其余五个进入绘制分支。 */
  tool: PinBoardTool;
  /** 图层可见性（下标 = layer）：界面据此渲染四个开关。 */
  visibleLayers: PinBoardLayerVisibility;
}

export interface PinBoardContextMenuRequest {
  clientX: number;
  clientY: number;
  selectedCount: number;
  arrangeableCount: number;
  /** 选中里含笔迹：图层菜单据此置灰（笔迹固定在标注层，不能被改层）。 */
  selectionHasStroke: boolean;
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
/**
 * 设备丢失后的自动重建上限与退避。上限是整个渲染器生命周期（≈ 一次会话）内
 * 允许重建的设备数，用于避免「丢设备 → 重建 → 又丢」退化成重建风暴——那正是
 * 最初观察到的冻结触发条件（短时间连续创建设备）。超限或重建失败即回到「只上报」。
 */
const GPU_RECOVERY_MAX_ATTEMPTS = 2;
const GPU_RECOVERY_BACKOFF_MS = 600;
const TRANSFORM_HANDLE_RADIUS = 12;
const ROTATION_HANDLE_OFFSET = 28;
const QUAD_TRIANGLE_ORDER = [0, 1, 3, 3, 1, 2] as const;

/** 指针拖拽的进行中状态；`draw` 是绘制工具下的那一支。 */
type DragMode = "pan" | "images" | "marquee" | "rotate" | "scale" | "draw";
/** 每种拖拽状态对应的画布光标类名。 */
const DRAG_CURSOR_CLASS: Record<DragMode, string> = {
  pan: "dragging",
  images: "moving-image",
  marquee: "selecting",
  rotate: "rotating-image",
  scale: "scaling-image",
  draw: "drawing",
};

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

/**
 * 笔迹着色器：整板笔迹合并成一条三角形列表一次画完。
 *
 * 每个顶点带「世界位置 + 局部坐标 + 图元参数」，片元在局部空间算距离场后按
 * `viewport.scale.x`（设备像素 / 世界单位）做 1 px 羽化。局部坐标到世界坐标的
 * 双线性映射在 CPU 侧完成（`strokeRender.ts`），因此移动、旋转、缩放不需要任何
 * 着色器分支。
 */
const STROKE_SHADER = `
struct Viewport {
  bounds: vec4f,
  // x = 设备像素 / 世界单位；yzw 保留。
  scale: vec4f
};
@group(0) @binding(0) var<uniform> viewport: Viewport;

struct StrokeVertex {
  @location(0) position: vec2f,
  @location(1) local: vec2f,
  @location(2) anchorA: vec2f,
  @location(3) anchorB: vec2f,
  @location(4) anchorC: vec2f,
  @location(5) radius: vec2f,
  @location(6) color: vec4f,
  @location(7) params: vec2f
};

struct StrokeVaryings {
  @builtin(position) position: vec4f,
  @location(0) local: vec2f,
  @location(1) anchorA: vec2f,
  @location(2) anchorB: vec2f,
  @location(3) anchorC: vec2f,
  @location(4) radius: vec2f,
  @location(5) color: vec4f,
  @location(6) params: vec2f
};

fn projected(position: vec2f) -> vec4f {
  let minPoint = viewport.bounds.xy;
  let extent = max(viewport.bounds.zw, vec2f(1.0));
  let normalized = (position - minPoint) / extent;
  return vec4f(normalized.x * 2.0 - 1.0, normalized.y * 2.0 - 1.0, 0.0, 1.0);
}

@vertex fn strokeVs(input: StrokeVertex) -> StrokeVaryings {
  var output: StrokeVaryings;
  output.position = projected(input.position);
  output.local = input.local;
  output.anchorA = input.anchorA;
  output.anchorB = input.anchorB;
  output.anchorC = input.anchorC;
  output.radius = input.radius;
  output.color = input.color;
  output.params = input.params;
  return output;
}

fn sdBox(p: vec2f, halfSize: vec2f) -> f32 {
  let d = abs(p) - halfSize;
  return length(max(d, vec2f(0.0))) + min(max(d.x, d.y), 0.0);
}

fn sdTriangle(p: vec2f, p0: vec2f, p1: vec2f, p2: vec2f) -> f32 {
  let e0 = p1 - p0;
  let e1 = p2 - p1;
  let e2 = p0 - p2;
  let v0 = p - p0;
  let v1 = p - p1;
  let v2 = p - p2;
  let pq0 = v0 - e0 * clamp(dot(v0, e0) / max(dot(e0, e0), 1e-8), 0.0, 1.0);
  let pq1 = v1 - e1 * clamp(dot(v1, e1) / max(dot(e1, e1), 1e-8), 0.0, 1.0);
  let pq2 = v2 - e2 * clamp(dot(v2, e2) / max(dot(e2, e2), 1e-8), 0.0, 1.0);
  let side = sign(e0.x * e2.y - e0.y * e2.x);
  let d0 = vec2f(dot(pq0, pq0), side * (v0.x * e0.y - v0.y * e0.x));
  let d1 = vec2f(dot(pq1, pq1), side * (v1.x * e1.y - v1.y * e1.x));
  let d2 = vec2f(dot(pq2, pq2), side * (v2.x * e2.y - v2.y * e2.x));
  let d = min(min(d0, d1), d2);
  return -sqrt(d.x) * sign(d.y);
}

@fragment fn strokeFs(input: StrokeVaryings) -> @location(0) vec4f {
  let kind = input.params.x;
  var dist: f32 = 0.0;
  if (kind < 0.5) {
    // 锥形胶囊：自由笔与直线，宽度在两端之间线性插值。
    let pa = input.local - input.anchorA;
    let ba = input.anchorB - input.anchorA;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-8), 0.0, 1.0);
    dist = length(pa - ba * h) - mix(input.radius.x, input.radius.y, h);
  } else if (kind < 1.5) {
    let center = (input.anchorA + input.anchorB) * 0.5;
    let halfSize = abs(input.anchorB - input.anchorA) * 0.5;
    dist = abs(sdBox(input.local - center, halfSize)) - input.radius.x;
  } else if (kind < 2.5) {
    let radii = max(input.anchorB, vec2f(1e-4));
    let scaled = (input.local - input.anchorA) / radii;
    dist = abs((length(scaled) - 1.0) * min(radii.x, radii.y)) - input.radius.x;
  } else {
    dist = sdTriangle(input.local, input.anchorA, input.anchorB, input.anchorC);
  }
  let feather = max(0.5 / max(viewport.scale.x, 1e-6), 1e-6);
  let coverage = clamp(1.0 - smoothstep(-feather, feather, dist), 0.0, 1.0);
  return vec4f(input.color.rgb, input.color.a * coverage);
}
`;

/** 视口 uniform 的字节数：`bounds` + `scale` 两个 vec4f。 */
const VIEWPORT_UNIFORM_BYTES = 32;

function cloneQuad(quad: Quad): Quad {
  return quad.map((point) => [...point] as Point) as Quad;
}

function cloneImage(image: PinBoardItem): PinBoardItem {
  return { ...image, points: cloneQuad(image.points), uv: cloneQuad(image.uv) };
}

function estimatedLoadBytes(image: PinBoardItem, maxDimension: number): number {
  if (image.kind === "text") {
    // 文字没有 DDS：负载是前端栅格化出的 RGBA 加头，另外留一份给 canvas 到
    // 上传缓冲的临时拷贝。
    return 12 + textTextureRgbaBytes(image, maxDimension) * 2;
  }
  return estimatedTextureLoadBytes(image, maxDimension);
}

function estimatedResidentBytes(image: PinBoardItem, maxDimension: number): number {
  if (image.kind === "text") return textTextureResidentBytes(image, maxDimension);
  // 笔迹是 SDF 几何，不占纹理预算。
  if (image.kind === "stroke") return 0;
  return estimatedTextureResidentBytes(image, maxDimension);
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
  image: Pick<PinBoardItem, "points" | "uv">,
  uvScale: readonly [number, number] = [1, 1],
): Float32Array {
  return new Float32Array(QUAD_TRIANGLE_ORDER.flatMap((index) => [
    image.points[index][0],
    image.points[index][1],
    image.uv[index][0] * uvScale[0],
    image.uv[index][1] * uvScale[1],
  ]));
}

function outlineData(image: Pick<PinBoardItem, "points">): Float32Array {
  return new Float32Array([0, 1, 2, 3, 0].flatMap((index) => image.points[index]));
}

function createGeometryBuffers(
  device: any,
  image: PinBoardItem,
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

/**
 * 诊断计数：`gpuDevicesCreated` 单调递增，`liveRenderers` 反映未销毁的渲染器。
 * 与 `destroy()` 的记录配对后可判断切换作品/画板时是否在累积 GPU 设备。
 */
let gpuDevicesCreated = 0;
let liveRenderers = 0;

/**
 * 每块 canvas 当前的 GPU 设备序号。同一块 canvas 会被反复 configure 到不同设备，
 * 销毁旧渲染器时只允许当前 owner 解绑上下文，避免旧设备影响新渲染器的 surface。
 */
const canvasGpuOwners = new WeakMap<HTMLCanvasElement, number>();

async function makeGpu(canvas: HTMLCanvasElement): Promise<GpuState> {
  const browserGpu = (navigator as any).gpu;
  if (!browserGpu) throw new Error("当前 WebView 不支持 WebGPU");
  const adapter = await browserGpu.requestAdapter();
  if (!adapter) throw new Error("无法初始化 WebGPU 适配器");
  if (!adapter.features.has("texture-compression-bc")) {
    throw new Error("当前显卡不支持 BC7 纹理");
  }

  const device = await adapter.requestDevice({ requiredFeatures: ["texture-compression-bc"] });
  // 设备序号在设备登记之前确定，使下面两个异步回调带上与实际登记一致的序号。
  const deviceSequence = gpuDevicesCreated + 1;
  // 设备丢失是显存压力或驱动重置的信号；主动 destroy 产生的 "destroyed" 不算异常。
  if (device.lost) {
    void device.lost.then((info: { reason?: string; message?: string }) => {
      const reason = info?.reason ?? "unknown";
      if (reason === "destroyed") return;
      diagnosticsLog(
        "warn",
        `[gpu] pin-board gpu device lost: seq=${deviceSequence}, reason=${reason}, message=${info?.message ?? ""}`,
      );
    });
  }
  // 未被错误作用域捕获的 GPU 错误默认被静默吞掉（异步校验失败、队列被回收等）。
  if ("onuncapturederror" in device) {
    device.onuncapturederror = (event: { error?: { message?: string } }) => {
      diagnosticsLog(
        "warn",
        `[gpu] pin-board gpu uncaptured error: seq=${deviceSequence}, message=${event?.error?.message ?? "unknown"}`,
      );
    };
  }
  const context = (canvas as any).getContext("webgpu");
  const format = browserGpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: "opaque" });
  const module = device.createShaderModule({ code: SHADER });
  const viewportLayout = device.createBindGroupLayout({
    entries: [{
      binding: 0,
      // 片元阶段也要可见：笔迹着色器按 `viewport.scale.x` 换算羽化宽度。
      visibility: 0x1 | 0x2,
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
  const strokeModule = device.createShaderModule({ code: STROKE_SHADER });
  const strokePipeline = device.createRenderPipeline({
    layout: device.createPipelineLayout({ bindGroupLayouts: [viewportLayout] }),
    vertex: {
      module: strokeModule,
      entryPoint: "strokeVs",
      buffers: [{
        arrayStride: STROKE_VERTEX_BYTES,
        attributes: [
          { shaderLocation: 0, offset: 0, format: "float32x2" },
          { shaderLocation: 1, offset: 8, format: "float32x2" },
          { shaderLocation: 2, offset: 16, format: "float32x2" },
          { shaderLocation: 3, offset: 24, format: "float32x2" },
          { shaderLocation: 4, offset: 32, format: "float32x2" },
          { shaderLocation: 5, offset: 40, format: "float32x2" },
          { shaderLocation: 6, offset: 48, format: "float32x4" },
          { shaderLocation: 7, offset: 64, format: "float32x2" },
        ],
      }],
    },
    fragment: {
      module: strokeModule,
      entryPoint: "strokeFs",
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
  const sampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
  const viewportBuffer = device.createBuffer({
    size: VIEWPORT_UNIFORM_BYTES,
    usage: 0x40 | 0x08,
  });
  const viewportBindGroup = device.createBindGroup({
    layout: viewportLayout,
    entries: [{ binding: 0, resource: { buffer: viewportBuffer } }],
  });
  gpuDevicesCreated = deviceSequence;
  canvasGpuOwners.set(canvas, gpuDevicesCreated);
  diagnosticsLog(
    "info",
    `pin-board gpu device created: seq=${gpuDevicesCreated}, liveRenderers=${liveRenderers}`,
  );
  return {
    canvas,
    device,
    context,
    pipeline,
    outlinePipeline,
    strokePipeline,
    sampler,
    viewportBuffer,
    viewportBindGroup,
    sequence: gpuDevicesCreated,
    released: false,
  };
}

async function loadTexture(
  gpu: GpuState,
  image: PinBoardItem,
  maxDimension: number,
): Promise<LoadedTexture> {
  // 位图读仓库内的 DDS；文字由前端按同一请求长边栅格化，随后共用 RGBA 上传路径。
  const response = image.kind === "text"
    ? textRgbaPayload(image, maxDimension)
    : await pinBoardApi.readPinBoardTexture(image.boardId, image.itemId, maxDimension);
  // 画板切换会先销毁旧设备，但旧 IPC 读取可能稍后才返回。不要让这个返回值继续
  // 触碰已经释放的 WebGPU 对象——此前会直接进入 createTexture/writeTexture，在
  // WebView2 的 GPU 路径上形成悬空设备调用。
  if (gpu.released) {
    diagnosticsLog(
      "warn",
      `texture.read completed after gpu release: gpuSeq=${gpu.sequence}, itemId=${image.itemId}, dim=${maxDimension}`,
    );
    throw new Error("纹理读取所属的 GPU 设备已释放");
  }
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
  private boardId = 0;
  private readonly viewport: Viewport = { ...DEFAULT_VIEWPORT };
  private images: PinBoardItem[] = [];
  private readonly imageById = new Map<number, PinBoardItem>();
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
  private visibleImages: PinBoardItem[] = [];
  private historyIndex = 0;
  private savedStateKey = "";
  private revision = "";
  private cssHeight = 0;
  private defaultWorldUnitsPerCssPixel = 1;
  private loadTimer: number | null = null;
  private loadRequested = false;
  private loading = false;
  private textureReservedBytes = 0;
  /** 已发起但尚未完成的纹理读取；用于销毁日志确认旧任务是否越过生命周期边界。 */
  private inFlightTextureLoads = 0;
  private generation = 0;
  private preservedWorldUnitsPerCssPixel: number | null = null;
  /** 打开画板时按图片最小包围框定位视图，等画布尺寸可用后再执行。 */
  private pendingViewportFit = false;
  /** 是否已装载画板。未装载时所有画板级绘制/保存/会话都停用，但 GPU 设备保持存活。 */
  private loaded = false;
  /** 设备丢失恢复：已尝试重建次数（不重置，作为整会话上限）。 */
  private gpuRecoveryAttempts = 0;
  /** GPU 不可用（丢失后正在重建，或已达重建上限而放弃）：期间暂停绘制与纹理加载。 */
  private gpuUnavailable = false;
  private recoveryTimer: number | null = null;
  private destroyed = false;
  private saving = false;
  private finalized = false;
  private active: boolean;
  private dirty = false;
  private autosaveTimer: number | null = null;
  private savePromise: Promise<boolean> | null = null;
  private finalizePromise: Promise<boolean> | null = null;
  private locked = false;
  private dragMode: DragMode | null = null;
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
  /**
   * 纹理未就绪的图片占位层：DOM 节点按 itemId 复用，避免每帧增删；位置随
   * 视口/图片旋转同步。纹理（缩略图或高清）到位后由下一次 `draw()` 移除。
   */
  private readonly placeholderNodes = new Map<number, HTMLElement>();
  private drawErrorReported = false;
  /** 当前画板的笔迹几何，按 itemId 索引；几何只走二进制通道，不进条目 DTO。 */
  private readonly strokeGeometries = new Map<number, PinBoardStrokeGeometry>();
  /** 笔迹缓冲的失效计数：几何或变换变化时递增，签名随之变化并触发重建。 */
  private strokeVersion = 0;
  /** 上次构建笔迹缓冲时的「可见笔迹 + 缩放」签名，避免平移时逐帧重建。 */
  private strokeSignature = "";
  /** 笔迹读取的代次：切画板或重复读取时丢弃旧结果。 */
  private strokeLoadToken = 0;
  private strokeVertexBuffer: any = null;
  private strokeIndexBuffer: any = null;
  private strokeIndexCount = 0;
  /** 可见笔迹的选中描边，按 `strokeOutlineIndex` 给出的槽位连续排布。 */
  private strokeOutlineBuffer: any = null;
  private readonly strokeOutlineIndex = new Map<number, number>();
  /** 当前工具：绘制工具下指针按下进入绘制分支，不触发选中/框选。 */
  private tool: PinBoardTool = "select";
  /**
   * 图层可见性：纯视图状态，随画板会话恢复，不写库也不标记 dirty。
   * 所有「哪些条目可画、可选、可点」的判断都经过它，少接一处就会出现
   * 「看不见但能选中」。
   */
  private visibleLayers: PinBoardLayerVisibility = normalizeLayerVisibility(DEFAULT_LAYER_VISIBILITY);
  /** 绘制样式由工具栏推入，落库与预览共用同一份值。 */
  private strokeColor = DEFAULT_STROKE_COLOR;
  private strokeWidthValue = DEFAULT_STROKE_WIDTH;
  /** 正在进行的绘制草稿（世界坐标）；`null` 表示没有绘制。 */
  private draft: StrokeDraft | null = null;

  private constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly selectionElement: HTMLElement,
    private readonly marqueeElement: HTMLElement,
    private readonly placeholderElement: HTMLElement,
    /** 绘制预览层：DOM 2D canvas，只画当前草稿，不进 GPU 缓冲。 */
    private readonly drawOverlay: HTMLCanvasElement,
    private gpu: GpuState,
    private arrangementGapRatio: number,
    private autosaveEnabled: boolean,
    private readonly onError: (message: string) => void,
    private readonly onState: (state: PinBoardInteractionState) => void,
    private readonly onSessionChange: (boardId: number, session: PinBoardViewSession) => void,
    private readonly onContextMenu: (request: PinBoardContextMenuRequest | null) => void,
    private readonly onStrokeCommit: (request: StrokeCommitRequest) => void,
    cacheBudgets: PinBoardTextureBudgets,
  ) {
    // 构造函数只负责画布生命周期内一次性初始化；画板数据由 `loadBoard` 装载。
    this.active = true;
    this.textureBudgetBytes = cacheBudgets.residentBytes;
    this.previewTextureBudgetBytes = cacheBudgets.previewBytes;
    const cssHeight = Math.max(canvas.getBoundingClientRect().height, 1);
    this.cssHeight = cssHeight;
    this.defaultWorldUnitsPerCssPixel = DEFAULT_VIEWPORT.height / cssHeight;
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
    canvas.addEventListener("pointercancel", this.pointerCancel);
    canvas.addEventListener("wheel", this.wheel, { passive: false });
    canvas.addEventListener("contextmenu", this.contextMenu);
    this.resize();

    this.armDeviceLossHandler(gpu);
  }

  /** 监听设备丢失（主动 destroy 产生的 "destroyed" 除外），触发限流后的自动重建。 */
  private armDeviceLossHandler(gpu: GpuState) {
    void gpu.device.lost.then((info: { reason?: string; message?: string }) => {
      if (this.destroyed || gpu.released || this.gpu !== gpu) return;
      if (info?.reason === "destroyed") return;
      this.handleDeviceLost(gpu, info);
    });
  }

  private handleDeviceLost(gpu: GpuState, info: { reason?: string; message?: string }) {
    // 旧设备的资源已作废：标记 released 让在飞行的纹理读取拒绝旧结果，并推进代次。
    // 同时挂起绘制与加载，直到重建完成（或确认放弃）。
    gpu.released = true;
    this.gpuUnavailable = true;
    this.generation += 1;
    const reason = info?.message || info?.reason || "未知原因";
    const attempt = this.gpuRecoveryAttempts + 1;
    if (this.gpuRecoveryAttempts >= GPU_RECOVERY_MAX_ATTEMPTS) {
      diagnosticsLog(
        "warn",
        `pin-board gpu device lost, recovery limit reached: gpuSeq=${gpu.sequence}, attempts=${this.gpuRecoveryAttempts}, reason=${reason}`,
      );
      this.onError(`WebGPU 设备已丢失且重建次数已达上限：${reason}`);
      return;
    }
    this.gpuRecoveryAttempts += 1;
    diagnosticsLog(
      "warn",
      `pin-board gpu device lost, scheduling recovery: gpuSeq=${gpu.sequence}, attempt=${attempt}/${GPU_RECOVERY_MAX_ATTEMPTS}, reason=${reason}`,
    );
    const delay = GPU_RECOVERY_BACKOFF_MS * attempt;
    this.recoveryTimer = window.setTimeout(() => {
      this.recoveryTimer = null;
      void this.recoverGpu();
    }, delay);
  }

  /** 重建 GPU 设备并重新装载纹理。与 `create` 共用 `makeGpu`，避免两条创建路径分叉。 */
  private async recoverGpu() {
    if (this.destroyed) return;
    try {
      // 旧设备上的纹理与缓冲全部作废，先丢弃（destroy 在失效设备上是无操作）。
      try {
        for (const loaded of this.textures.values()) destroyTexture(loaded);
        for (const loaded of this.previewTextures.values()) destroyTexture(loaded);
        this.gpu.viewportBuffer.destroy();
      } catch {
        // 设备已失效时忽略释放失败。
      }
      this.textures.clear();
      this.previewTextures.clear();
      this.textureReservedBytes = 0;
      this.failedTextureDimensions.clear();
      this.textureQualityCaps.clear();
      // 笔迹缓冲也属于旧设备：清空后由 refreshViewport 按当前画板重建。
      this.destroyStrokeBuffers();
      this.strokeSignature = "";

      const next = await makeGpu(this.canvas);
      if (this.destroyed) {
        next.released = true;
        try {
          next.device.destroy();
        } catch {
          // 忽略。
        }
        return;
      }
      this.gpu = next;
      this.armDeviceLossHandler(next);
      this.gpuUnavailable = false;
      diagnosticsLog(
        "info",
        `pin-board gpu device recovered: newSeq=${next.sequence}, boardId=${this.boardId}, attempts=${this.gpuRecoveryAttempts}`,
      );
      if (this.loaded) this.refreshViewport(0);
      else this.draw();
    } catch (error) {
      // 重建失败时保持 GPU 不可用（挂起绘制），避免持续对失效设备绘图刷错误。
      diagnosticsLog("warn", `pin-board gpu recovery failed: error=${errorMessage(error)}`);
      this.onError(`素材板 GPU 恢复失败：${errorMessage(error)}`);
    }
  }

  /**
   * 创建渲染器：只做画布生命周期内一次性的 GPU 初始化。画板数据随后由 `loadBoard`
   * 装载，因此切换画板/作品不再销毁并新建 GPU 设备。
   */
  static async create(
    canvas: HTMLCanvasElement,
    selectionElement: HTMLElement,
    marqueeElement: HTMLElement,
    placeholderElement: HTMLElement,
    drawOverlay: HTMLCanvasElement,
    arrangementGapRatio: number,
    autosaveEnabled: boolean,
    onError: (message: string) => void,
    onState: (state: PinBoardInteractionState) => void,
    onSessionChange: (boardId: number, session: PinBoardViewSession) => void,
    onContextMenu: (request: PinBoardContextMenuRequest | null) => void,
    onStrokeCommit: (request: StrokeCommitRequest) => void,
    cacheBudgets: PinBoardTextureBudgets = textureBudgetsForLevel(undefined),
  ): Promise<PinBoardRenderer> {
    const gpu = await makeGpu(canvas);
    const renderer = new PinBoardRenderer(
      canvas,
      selectionElement,
      marqueeElement,
      placeholderElement,
      drawOverlay,
      gpu,
      arrangementGapRatio,
      autosaveEnabled,
      onError,
      onState,
      onSessionChange,
      onContextMenu,
      onStrokeCommit,
      cacheBudgets,
    );
    liveRenderers += 1;
    return renderer;
  }

  /** 当前装载的画板 id；未装载时为 0。 */
  get currentBoardId(): number {
    return this.boardId;
  }

  /**
   * 装载一块画板：先把旧画板的未保存改动导出成快照交给后台结算队列，随后**同步**释放
   * 旧画板并装载新画板。
   *
   * 切换因此不等待任何仓库写入：后台任务（维护扫描、恢复、精简）持写锁多久都不影响这里。
   * 旧画板的改动由 `pendingSettle` 的串行 worker 写回；同一 `boardId` 幂等。传入的
   * boardId 与当前一致时直接返回。
   *
   * `stagePrevious = false` 用于旧画板已被删除或移入回收站、不能再次写库的场景（那里的
   * 内存状态直接丢弃）。
   */
  loadBoard(
    view: PinBoardView,
    session: PinBoardViewSession | null,
    stagePrevious = true,
  ): boolean {
    if (this.destroyed) return false;
    if (this.loaded && this.boardId === view.boardId) return true;
    this.detachCurrentBoard(stagePrevious);
    if (this.destroyed) return false;
    this.activateBoard(view, session);
    return true;
  }

  /** 卸载当前画板（当前作品的画板已被清空等场景）：导出待结算快照后清空画面，保留 GPU 设备。 */
  unloadBoard(stagePrevious = true): void {
    if (this.destroyed || !this.loaded) return;
    this.detachCurrentBoard(stagePrevious);
    this.locked = false;
    this.canvas.classList.remove("locked");
    this.selectionElement.hidden = true;
    this.marqueeElement.hidden = true;
    this.selectionScreenQuad = null;
    this.draw();
    this.onState(this.interactionState);
  }

  /**
   * 释放当前画板：先记视图会话、再把未保存的改动导出成快照，最后丢画板级资源。
   *
   * 导出是同步的（快照就是内存里条目的副本），因此从调用方看切换是瞬时的。
   */
  private detachCurrentBoard(stagePending: boolean) {
    if (!this.loaded) return;
    this.finishActiveInteraction();
    this.persistSession();
    // 挂起的自动保存由快照接手：队列会写同一份内容，不必再走一次防抖保存。
    this.cancelAutosave();
    if (stagePending) this.stagePendingBoard();
    this.releaseBoardResources();
    this.loaded = false;
    this.boardId = 0;
  }

  /**
   * 把未保存的改动交给后台结算队列。
   *
   * 没有未保存改动时不产生快照——切一次作品不该在数据层变成一次仓库写操作，这也是
   * 「后台任务期间照样能切」的来源。软删除条目的回收由队列在保存成功后补做
   * （`finalize_pin_board`），不在切换路径上。
   */
  private stagePendingBoard() {
    if (!this.dirty) return;
    stageBoardSettle(
      {
        boardId: this.boardId,
        revision: this.revision,
        items: this.images.map(cloneImage),
      },
      // 自动保存可能正在飞行：队列要等它落地再写，否则会拿着旧 revision 被拒。
      this.savePromise ?? undefined,
    );
    diagnosticsLog(
      "info",
      `pin-board pending staged: boardId=${this.boardId}, items=${this.images.length}, saving=${this.savePromise !== null}`,
    );
  }

  /** 释放画板级资源：纹理、历史、选中、拖动中间态与飞行中的加载。 */
  private releaseBoardResources() {
    if (this.loadTimer !== null) {
      window.clearTimeout(this.loadTimer);
      this.loadTimer = null;
    }
    this.loadRequested = false;
    for (const loaded of this.textures.values()) destroyTexture(loaded);
    this.textures.clear();
    for (const loaded of this.previewTextures.values()) destroyTexture(loaded);
    this.previewTextures.clear();
    this.textureReservedBytes = 0;
    this.failedTextureDimensions.clear();
    this.textureQualityCaps.clear();
    this.clearPlaceholderOverlay();
    // 绘制草稿属于画板：换画板、卸载与销毁都不该留下半笔。
    this.draft = null;
    this.clearStrokePreview();
    this.releaseStrokeResources();
    this.visibleImages = [];
    this.history.length = 0;
    this.historyIndex = 0;
    this.images = [];
    this.imageById.clear();
    this.selectedIds.clear();
    this.dragMode = null;
    this.dragPointerId = null;
    this.dragBefore = [];
    this.marqueeBaseIds.clear();
    // 递增代次，让旧画板在飞行的纹理加载在返回时按代次被丢弃。
    this.generation += 1;
  }

  /**
   * 释放笔迹几何与 GPU 缓冲。读取代次递增，使在途的 `read_pin_board_strokes`
   * 返回后按代次被丢弃——它读的是上一块画板的几何。
   */
  private releaseStrokeResources() {
    this.strokeLoadToken += 1;
    this.strokeGeometries.clear();
    this.strokeVersion += 1;
    this.destroyStrokeBuffers();
    this.strokeSignature = "";
  }

  private destroyStrokeBuffers() {
    this.strokeVertexBuffer?.destroy();
    this.strokeIndexBuffer?.destroy();
    this.strokeOutlineBuffer?.destroy();
    this.strokeVertexBuffer = null;
    this.strokeIndexBuffer = null;
    this.strokeOutlineBuffer = null;
    this.strokeIndexCount = 0;
    this.strokeOutlineIndex.clear();
  }

  /**
   * 读取整板笔迹几何（`read_pin_board_strokes` 的二进制载荷），一次解析成
   * `Map<itemId, StrokeGeometry>`，随后重建笔迹缓冲。
   *
   * 与画板装载解耦：`loadBoard` 保持同步，笔迹在 IPC 回来后就地补画，因此切换画板
   * 不会为了几 KB 几何数据多等一次往返。
   */
  private async loadStrokes(boardId: number) {
    const token = ++this.strokeLoadToken;
    try {
      const response = await pinBoardApi.readPinBoardStrokes(boardId);
      const bytes = response instanceof Uint8Array ? response : new Uint8Array(response as ArrayBuffer);
      const strokes = parseStrokeBatch(bytes);
      if (this.destroyed || this.boardId !== boardId || token !== this.strokeLoadToken) return;
      this.strokeGeometries.clear();
      for (const [itemId, geometry] of strokes) this.strokeGeometries.set(itemId, geometry);
      this.strokeVersion += 1;
      this.strokeSignature = "";
      diagnosticsLog(
        "info",
        `pin-board strokes loaded: boardId=${boardId}, strokes=${strokes.size}, bytes=${bytes.byteLength}`,
      );
      this.refreshViewport(0);
      // 新笔迹已经进缓冲：等落库的预览到这一步才撤，中途不会闪空白。
      this.clearPendingStrokePreview();
    } catch (error) {
      if (this.destroyed || this.boardId !== boardId || token !== this.strokeLoadToken) return;
      diagnosticsLog("warn", `pin-board strokes failed: boardId=${boardId}, error=${errorMessage(error)}`);
      this.onError(`笔迹读取失败：${errorMessage(error)}`);
    }
  }

  /** 设备像素 / 世界单位：抗锯齿羽化与笔迹 LOD 都按它换算。 */
  private devicePixelsPerWorldUnit(): number {
    return this.canvas.height / Math.max(this.viewport.height, 1e-6);
  }

  /**
   * 按「可见笔迹 + 缩放」重建笔迹缓冲。
   *
   * 签名不变就跳过：平移只改视口位置、不改可见集合与缩放，因此拖动画布不会逐帧
   * 重建整板笔迹（逐段裁剪留给阶段 5 的性能优化）。
   */
  private syncStrokeBuffers() {
    const strokes = this.visibleImages.filter(
      (image): image is PinBoardStrokeItem => image.kind === "stroke",
    );
    const pixelsPerWorldUnit = this.devicePixelsPerWorldUnit();
    const signature = [
      this.strokeVersion,
      Math.round(pixelsPerWorldUnit * 100),
      strokes.map((stroke) => stroke.itemId).join(","),
    ].join("|");
    if (signature === this.strokeSignature) return;
    this.strokeSignature = signature;
    const entries = strokes.flatMap((item) => {
      const geometry = this.strokeGeometries.get(item.itemId);
      return geometry ? [{ item, geometry }] : [];
    });
    const batch = buildStrokeBatch(entries, { pixelsPerWorldUnit });
    this.uploadStrokeBatch(batch, buildStrokeOutlines(strokes));
  }

  private uploadStrokeBatch(
    batch: StrokeBatch,
    outlines: { vertices: Float32Array; index: Map<number, number> },
  ) {
    this.destroyStrokeBuffers();
    if (batch.indices.length === 0) return;
    try {
      const device = this.gpu.device;
      // 先挂到字段上再写数据：中途失败时 `destroyStrokeBuffers` 仍能释放已创建的缓冲。
      const vertexBuffer = device.createBuffer({
        size: batch.vertices.byteLength,
        usage: 0x20 | 0x08,
        mappedAtCreation: true,
      });
      this.strokeVertexBuffer = vertexBuffer;
      new Float32Array(vertexBuffer.getMappedRange()).set(batch.vertices);
      vertexBuffer.unmap();
      const indexBuffer = device.createBuffer({
        size: batch.indices.byteLength,
        usage: 0x10 | 0x08,
        mappedAtCreation: true,
      });
      this.strokeIndexBuffer = indexBuffer;
      new Uint32Array(indexBuffer.getMappedRange()).set(batch.indices);
      indexBuffer.unmap();
      this.strokeIndexCount = batch.indices.length;
      for (const [itemId, slot] of outlines.index) this.strokeOutlineIndex.set(itemId, slot);
      if (outlines.vertices.length > 0) {
        const outlineBuffer = device.createBuffer({
          size: outlines.vertices.byteLength,
          usage: 0x20 | 0x08,
          mappedAtCreation: true,
        });
        this.strokeOutlineBuffer = outlineBuffer;
        new Float32Array(outlineBuffer.getMappedRange()).set(outlines.vertices);
        outlineBuffer.unmap();
      }
    } catch (error) {
      this.destroyStrokeBuffers();
      diagnosticsLog(
        "warn",
        `pin-board stroke buffer failed: boardId=${this.boardId}, error=${errorMessage(error)}`,
      );
      this.onError(`笔迹缓冲创建失败：${errorMessage(error)}`);
    }
  }

  private activateBoard(view: PinBoardView, session: PinBoardViewSession | null) {
    // 该画板若还有后台未写回的快照，内容以快照为准：它比仓库里的更新（用户刚在这块
    // 画板上编辑过）；否则后台正握着写、这边又按旧内容装载，界面会回退。
    const pending = takeBoardSettle(view.boardId);
    this.boardId = view.boardId;
    this.images = (pending?.items ?? view.items).map(cloneImage);
    this.imageById.clear();
    for (const image of this.images) this.imageById.set(image.itemId, image);
    this.revision = resolveBoardRevision(view.boardId, pending?.revision ?? view.revision);
    // 装载到的 revision 是仓库当前事实：记进账本，避免账本里残留的旧值在装载
    // 之后继续压过它（例如上次保存撞了冲突、之后重新打开画板时）。
    recordBoardRevision(view.boardId, this.revision);
    this.finalized = false;
    this.finalizePromise = null;
    this.savePromise = null;
    this.saving = false;
    // 快照还没落库，改动就必须继续算「未保存」，否则它会在下次切换时被丢掉。
    this.dirty = pending !== null;
    this.cancelAutosave();
    this.cssHeight = Math.max(this.canvas.getBoundingClientRect().height, 1);
    this.viewport.centerX = DEFAULT_VIEWPORT.centerX;
    this.viewport.centerY = DEFAULT_VIEWPORT.centerY;
    this.viewport.height = DEFAULT_VIEWPORT.height;
    this.pendingViewportFit = false;
    this.locked = false;
    this.selectedIds.clear();
    // 图层可见性要先于选中与视图适配恢复：`contentBounds` 与选中过滤都依赖它，
    // 否则「打开画板 → 按隐藏层里的图片适配视图」会把视图缩放到看不见的内容上。
    this.visibleLayers = normalizeLayerVisibility(session?.visibleLayers);
    // 笔迹几何随画板重新读取：`releaseBoardResources` 已释放旧缓冲，这里再兜一次，
    // 保证直接 `activateBoard`（未经 detach）时也不会画出上一块画板的笔迹。
    this.releaseStrokeResources();
    if (session) {
      this.viewport.centerX = session.centerX;
      this.viewport.centerY = session.centerY;
      this.viewport.height = this.clampViewportHeight(
        session.worldUnitsPerCssPixel * this.cssHeight,
      );
      this.locked = session.locked;
      if (!this.locked) {
        for (const itemId of session.selectedImageIds) {
          const item = this.imageById.get(itemId);
          // 隐藏层里的条目不能被恢复成选中：会话存的是当时的选择，图层开关是之后改的。
          if (item && isLayerVisible(this.visibleLayers, item.layer)) {
            this.selectedIds.add(itemId);
          }
        }
      }
    } else {
      // 进程内没有该画板的会话：打开时按画板内全部图片的最小包围框定位视图。
      this.locked = true;
      this.pendingViewportFit = true;
    }
    // 取回快照时不能把当前状态记成「已保存」：否则下一次 `save` 会直接短路，
    // 快照里的改动就永远落不了库。
    this.savedStateKey = pending ? "" : this.stateKey();
    this.loaded = true;
    this.resize();
    if (this.images.some((image) => image.kind === "stroke")) {
      void this.loadStrokes(view.boardId);
    }
    diagnosticsLog(
      "info",
      pending
        ? `pin-board board loaded from pending snapshot: boardId=${view.boardId}, images=${this.images.length}`
        : `pin-board board loaded: boardId=${view.boardId}, images=${this.images.length}`,
    );
  }

  /** 发出当前画板的视图会话（按 boardId 归属）。 */
  private persistSession() {
    this.onSessionChange(this.boardId, {
      centerX: this.viewport.centerX,
      centerY: this.viewport.centerY,
      worldUnitsPerCssPixel: this.worldUnitsPerCssPixel(),
      locked: this.locked,
      selectedImageIds: this.locked ? [] : this.selectedImageIds(),
      visibleLayers: normalizeLayerVisibility(this.visibleLayers),
    });
  }

  /**
   * 释放整个渲染器与 GPU 设备。只用于模块真正卸载（关闭工作区、切仓库、退出），
   * 切换画板/作品请改用 `loadBoard` / `unloadBoard`。
   *
   * `stagePending` 为真时把未保存的改动交给后台结算队列：模块卸载（例如切到非作品节点）
   * 没有等待写入的时机，交给队列比一次可能失败且无人可见的写更可靠——重新打开该画板时
   * 快照会被 `takeBoardSettle` 取回。
   */
  destroy(stagePending = true) {
    if (this.destroyed) return;
    this.cancelAutosave();
    if (stagePending && this.loaded && !this.finalized) this.stagePendingBoard();
    if (this.loaded) this.persistSession();
    this.destroyed = true;
    this.generation += 1;
    if (this.loadTimer !== null) window.clearTimeout(this.loadTimer);
    if (this.recoveryTimer !== null) {
      window.clearTimeout(this.recoveryTimer);
      this.recoveryTimer = null;
    }
    this.resizeObserver.disconnect();
    this.themeObserver.disconnect();
    window.removeEventListener("resize", this.resize);
    window.removeEventListener("keydown", this.keyDown);
    window.removeEventListener("pagehide", this.autoSave);
    document.removeEventListener("visibilitychange", this.visibilityChange);
    this.canvas.removeEventListener("pointerdown", this.pointerDown);
    this.canvas.removeEventListener("pointermove", this.pointerMove);
    this.canvas.removeEventListener("pointerup", this.pointerUp);
    this.canvas.removeEventListener("pointercancel", this.pointerCancel);
    this.canvas.removeEventListener("wheel", this.wheel);
    this.canvas.removeEventListener("contextmenu", this.contextMenu);
    this.clearPlaceholderOverlay();
    this.draft = null;
    this.clearStrokePreview();
    this.releaseStrokeResources();
    for (const loaded of this.textures.values()) destroyTexture(loaded);
    this.textures.clear();
    for (const loaded of this.previewTextures.values()) destroyTexture(loaded);
    this.previewTextures.clear();
    this.textureReservedBytes = 0;
    this.gpu.viewportBuffer.destroy();
    // 先标记设备已释放，再解绑 canvas 上下文并销毁设备。在飞行的纹理读取返回后，
    // loadTexture 会在任何 GPU 调用之前拒绝旧结果。
    this.gpu.released = true;
    if (canvasGpuOwners.get(this.gpu.canvas) === this.gpu.sequence) {
      try {
        this.gpu.context.unconfigure();
      } catch {
        // 上下文已失效时忽略。
      }
      canvasGpuOwners.delete(this.gpu.canvas);
    } else {
      diagnosticsLog(
        "info",
        `gpu unconfigure skipped for stale owner: gpuSeq=${this.gpu.sequence}, currentOwner=${canvasGpuOwners.get(this.gpu.canvas) ?? "none"}`,
      );
    }
    try {
      this.gpu.device.destroy();
    } catch {
      // 设备已丢失时忽略。
    }
    liveRenderers = Math.max(0, liveRenderers - 1);
    diagnosticsLog(
      "info",
      `pin-board renderer destroyed: gpuSeq=${this.gpu.sequence}, liveRenderers=${liveRenderers}, finalized=${this.finalized}, stagePending=${stagePending}, inFlightTextureLoads=${this.inFlightTextureLoads}`,
    );
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

  /** 阵列间距：相对统一图片高度的比例，`0` 表示图片紧贴。 */
  setArrangementGapRatio(ratio: number) {
    this.arrangementGapRatio = ratio;
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

  /**
   * 切换工具。切到绘制工具时清空选中与框选残留：绘制中不该继续显示变换手柄，
   * 也不该把上一轮的拖拽快照带进来。锁定时只能停在 `select`。
   */
  setTool(tool: PinBoardTool) {
    if (!this.loaded || this.locked || this.tool === tool) return;
    this.finishActiveInteraction();
    this.tool = tool;
    this.dragBefore = [];
    this.marqueeBaseIds.clear();
    this.updateMarqueeOverlay(null);
    if (isStrokeTool(tool)) this.selectedIds.clear();
    // 选绘制工具时自动打开标注层：隐藏着还让人画，画完什么也看不见。
    if (isStrokeTool(tool) && !isLayerVisible(this.visibleLayers, STROKE_LAYER)) {
      this.setLayerVisible(STROKE_LAYER, true);
    }
    this.refreshSelectionVisuals();
  }

  /**
   * 开关一个图层。纯视图状态：不写库、不标记 dirty、不进撤销栈。
   *
   * 隐藏某层时把该层里正被选中的条目一起踢出选中，否则它们看不见却还能被删除、
   * 导出或改图层。
   */
  setLayerVisible(layer: number, visible: boolean) {
    if (!this.loaded) return;
    const next = normalizeLayerVisibility(this.visibleLayers);
    if (!Number.isInteger(layer) || layer < 0 || layer >= PIN_BOARD_LAYER_COUNT) return;
    if (next[layer] === visible) return;
    next[layer] = visible;
    this.visibleLayers = next;
    if (!visible) {
      for (const itemId of hiddenSelectionIds(this.selectedIds, this.images, next)) {
        this.selectedIds.delete(itemId);
      }
    }
    this.refreshViewport(0);
    this.emitState();
  }

  /** 绘制样式：只影响之后画出的笔迹，不改已有条目。 */
  setStrokeStyle(color: string, width: number) {
    this.strokeColor = /^#[0-9a-f]{6}$/i.test(color) ? color.toLowerCase() : DEFAULT_STROKE_COLOR;
    this.strokeWidthValue = clampStrokeWidth(width);
    if (this.draft) this.drawStrokePreview();
  }

  /**
   * 取消进行中的绘制。不产生条目，也不留预览：锁定、切换画板与释放指针捕获
   * 都走这一条路径。
   */
  cancelDrawing() {
    if (!this.draft && this.dragMode !== "draw") return;
    this.draft = null;
    this.clearStrokePreview();
    this.releaseDrawingPointer();
  }

  /**
   * 清掉「已提交、等落库」的预览。
   *
   * 落库要经过保存、创建与重读几何几个往返，pointerUp 时立刻清掉预览会闪一下空白，
   * 所以预览一直留到新笔迹真正进缓冲为止（`loadStrokes`）或提交失败为止。
   * 正在画下一笔时不动：那一笔的预览已经取代了它。
   */
  clearPendingStrokePreview() {
    if (this.dragMode === "draw" || !this.draft) return;
    this.draft = null;
    this.clearStrokePreview();
  }

  beginViewportResize() {
    this.preservedWorldUnitsPerCssPixel = this.worldUnitsPerCssPixel();
  }

  endViewportResize() {
    this.resize();
    this.preservedWorldUnitsPerCssPixel = null;
  }

  get interactionState(): PinBoardInteractionState {
    const selected = this.selectedImages();
    return {
      imageCount: this.images.filter((image) => !image.deleted).length,
      selectedCount: selected.length,
      arrangeableCount: selected.filter((image) => image.kind === "bitmap").length,
      exportableCount: selected.filter((image) => image.kind !== "stroke").length,
      canUndo: !this.locked && this.historyIndex > 0,
      canRedo: !this.locked && this.historyIndex < this.history.length,
      dirty: this.dirty,
      saving: this.saving,
      locked: this.locked,
      zoomPercent: Math.round((
        this.defaultWorldUnitsPerCssPixel / this.worldUnitsPerCssPixel()
      ) * 100),
      tool: this.tool,
      visibleLayers: normalizeLayerVisibility(this.visibleLayers),
    };
  }

  get boardRevision(): string {
    return this.revision;
  }

  get placementPoint(): Point {
    return this.cursorWorld ? [...this.cursorWorld] as Point : [this.viewport.centerX, this.viewport.centerY];
  }

  copySelection(): PinBoardClipboardImage[] {
    if (this.locked) return [];
    // 画板内复制/粘贴只承载位图；文字条目当前没有可复制的载荷通道。
    return this.selectedImages()
      .filter((image) => image.kind === "bitmap")
      .sort((left, right) => left.order - right.order)
      .map((image) => ({
        displayWidth: imageSize(image)[0],
        displayHeight: imageSize(image)[1],
        sourceBoardId: image.boardId,
        sourceItemId: image.itemId,
        width: image.width,
        height: image.height,
        uv: cloneQuad(image.uv),
      }));
  }

  selectedTextItems(): PinBoardTextItem[] {
    return this.selectedImages().filter((item): item is PinBoardTextItem => item.kind === "text");
  }

  selectedImageIds(): number[] {
    return this.selectedImages().map((image) => image.itemId);
  }

  /**
   * 选中画板内全部未删除图片（Ctrl+A 全选整个画布）。锁定时不生效；选中态不属于
   * 模型状态，所以这里不调整图片顺序、也不标记 dirty。
   */
  selectAll() {
    if (this.locked) return;
    this.finishActiveInteraction();
    this.selectedIds.clear();
    for (const image of this.images) {
      // 隐藏图层里的条目不能被全选：看不见却被选中，之后的删除/导出会莫名其妙地作用到它们。
      if (image.deleted || !isLayerVisible(this.visibleLayers, image.layer)) continue;
      this.selectedIds.add(image.itemId);
    }
    this.refreshSelectionVisuals();
  }

  /** 拖放导入时把放置点对齐到鼠标落点，避免沿用上一次悬停位置。 */
  setPlacementFromClient(clientX: number, clientY: number) {
    this.cursorWorld = screenToWorld(
      clientX,
      clientY,
      this.canvas.getBoundingClientRect(),
      this.viewport,
    );
  }

  copyImageToClipboardId(): number | null {
    const selected = this.selectedImages();
    return selected.length === 1 && selected[0].kind === "bitmap" ? selected[0].itemId : null;
  }

  deleteSelected() {
    const selected = this.selectedImages();
    if (this.locked || selected.length === 0) return;
    const itemIds = selected.map((image) => image.itemId);
    const before = this.snapshot(itemIds);
    for (const image of selected) image.deleted = true;
    const after = this.snapshot(itemIds);
    this.pushHistory(before, after);
    this.selectedIds.clear();
    this.afterModelChange(itemIds);
  }

  applyMutationResult(view: PinBoardView, itemIds: number[]) {
    if (!this.loaded || view.boardId !== this.boardId) return;
    const added = itemIds.flatMap((itemId) => view.items.filter((image) => image.itemId === itemId));
    if (added.length === 0) return;
    for (const image of view.items) {
      const current = this.imageById.get(image.itemId);
      if (current) {
        current.order = image.order;
        current.layer = image.layer;
        current.deleted = image.deleted;
      }
    }
    const before = added.map((image) => ({
      itemId: image.itemId,
      deleted: true,
      layer: image.layer,
      points: cloneQuad(image.points),
      uv: cloneQuad(image.uv),
    }));
    // 新条目落进隐藏图层会让「添加成功」看起来像什么都没发生（文字、粘贴、导入
    // 都会撞上），因此把它们所在的层一并打开——刚创建的内容应该看得见。
    const hiddenLayers = new Set(
      added
        .map((image) => image.layer)
        .filter((layer) => !isLayerVisible(this.visibleLayers, layer)),
    );
    if (hiddenLayers.size > 0) {
      const next = normalizeLayerVisibility(this.visibleLayers);
      for (const layer of hiddenLayers) next[layer] = true;
      this.visibleLayers = next;
    }
    for (const image of added) {
      const copy = cloneImage(image);
      this.images.push(copy);
      this.imageById.set(copy.itemId, copy);
      // 绘制工具下不自动选中新条目：工具模式的指针只走绘制分支，包围框既拖不动、
      // 也挡在下一笔落笔的位置上。只有回到选择工具才让新条目可变换。
      if (this.tool === "select") this.selectedIds.add(copy.itemId);
    }
    const after = this.snapshot(itemIds);
    this.revision = view.revision;
    // 新建命令（笔迹/文字/粘贴/导入）会推进后端 revision。这里必须同步推进共享
    // 账本，否则下一次保存仍从账本取旧值，被后端判成「画板已被其他窗口修改」。
    recordBoardRevision(this.boardId, view.revision);
    this.savedStateKey = this.stateKey();
    this.pushHistory(before, after);
    this.afterModelChange(itemIds);
    // 新笔迹的几何不在条目 DTO 里：落库后重读一次整板几何再重建缓冲。
    if (added.some((image) => image.kind === "stroke")) void this.loadStrokes(this.boardId);
  }

  toggleLock() {
    if (!this.locked) {
      this.finishInteraction();
      this.selectedIds.clear();
      this.onContextMenu(null);
    }
    this.locked = !this.locked;
    // 锁定时绘制工具一并退出：工具栏上的工具按钮此时是禁用的，停留在「笔」上
    // 只会让人以为还能画。
    if (this.locked) this.tool = "select";
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
    this.afterModelChange(entry.before.map((image) => image.itemId));
  }

  redo() {
    if (this.locked || this.historyIndex >= this.history.length) return;
    const entry = this.history[this.historyIndex];
    this.historyIndex += 1;
    this.restore(entry.after);
    this.afterModelChange(entry.after.map((image) => image.itemId));
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
    // 笔迹固定落在标注层，保存时原生端会忽略前端发来的 layer；本地跟着改只会
    // 变成「改完重开又回去」。混选时整组拒绝而不是「只改图片」——半执行的图层
    // 切换没有可见反馈，用户会以为笔迹也跟着走了。
    if (this.selectedImages().some((image) => image.kind === "stroke")) return;
    this.commitMutation((image) => {
      image.layer = layer;
    });
  }

  /**
   * 阵列排序：只处理位图条目。
   *
   * 几何计算统一在 Rust 侧完成（`arrange_pin_board_images` 是纯计算命令，不写库），
   * 这里只负责收集输入、把结果交回 `commitMutation`。撤销栈、脏标记与自动保存因此
   * 与其它几何操作完全一致。文字条目被排除：它的 quad 被拉伸时 `fontSize` 不会跟着
   * 变，字会被拉变形。
   */
  async arrangeSelected(): Promise<void> {
    if (this.locked) return;
    this.finishActiveInteraction();
    const bitmaps = this.selectedImages().filter((image) => image.kind === "bitmap");
    if (bitmaps.length < 2) return;
    const items = bitmaps.map((image) => {
      const [width, height] = imageSize(image);
      const [centerX, centerY] = quadCenter(image.points);
      return { itemId: image.itemId, width, height, centerX, centerY };
    });
    const boardId = this.boardId;
    let arranged: PinBoardArrangeResult[];
    try {
      arranged = await pinBoardApi.arrangePinBoardImages(items, this.arrangementGapRatio);
    } catch (error) {
      diagnosticsLog("error", `pin-board arrange failed: ${errorMessage(error)}`);
      this.onError(errorMessage(error));
      return;
    }
    // 计算是异步的：结果回来时画板可能已经切换，落位必须丢弃。
    if (this.boardId !== boardId || this.destroyed || this.locked) return;
    const points = new Map(arranged.map((entry) => [entry.itemId, entry.points]));
    this.commitMutation((image) => {
      const next = points.get(image.itemId);
      if (next) image.points = next;
    });
  }

  save(): Promise<boolean> {
    this.cancelAutosave();
    if (!this.loaded) return Promise.resolve(true);
    if (this.savePromise) {
      return this.savePromise.then((saved) => (
        saved && this.dirty ? this.save() : saved
      ));
    }
    const stateKey = this.stateKey();
    if (stateKey === this.savedStateKey) return Promise.resolve(true);
    // 切换画板是同步释放的，写入却可能还在飞行：结果回来时当前画板可能已经换了一块，
    // 因此保存期间的变化一律按发起时的 boardId 归属，只推进 revision 账本。
    const boardId = this.boardId;
    const revision = resolveBoardRevision(boardId, this.revision);
    const images = this.editableImages();
    this.saving = true;
    this.emitState();
    const started = performance.now();
    diagnosticsLog(
      "info",
      `pin-board save start: boardId=${boardId}, revision=${revision}, images=${images.length}`,
    );
    const task = pinBoardApi.savePinBoard(boardId, images, revision)
      .then((result) => {
        recordBoardRevision(boardId, result.revision);
        if (this.boardId !== boardId) {
          diagnosticsLog(
            "info",
            `pin-board save landed on a released board: boardId=${boardId}, revision=${result.revision}, elapsedMs=${Math.round(performance.now() - started)}`,
          );
          return true;
        }
        this.revision = result.revision;
        this.savedStateKey = stateKey;
        this.dirty = this.stateKey() !== stateKey;
        // 保存期间又有编辑时，恢复自动保存排程。
        if (this.dirty) this.scheduleAutosave();
        diagnosticsLog(
          "info",
          `pin-board save done: boardId=${boardId}, revision=${result.revision}, elapsedMs=${Math.round(performance.now() - started)}`,
        );
        return true;
      })
      .catch((error) => {
        diagnosticsLog(
          "warn",
          `pin-board save failed: boardId=${boardId}, elapsedMs=${Math.round(performance.now() - started)}, error=${errorMessage(error)}`,
        );
        // 已释出的画板没有界面可提示：它的改动在后台队列里，由队列决定重试或报错。
        if (this.boardId === boardId) this.onError(`画板保存失败：${errorMessage(error)}`);
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
    if (!this.loaded) return Promise.resolve(true);
    if (this.finalized) return Promise.resolve(true);
    if (this.finalizePromise) return this.finalizePromise;
    const boardId = this.boardId;
    diagnosticsLog(
      "info",
      `pin-board finalize start: boardId=${boardId}, revision=${this.revision}`,
    );
    const task = this.save()
      .then((saved) => {
        if (!saved) {
          diagnosticsLog(
            "warn",
            `pin-board finalize aborted: boardId=${boardId}, save failed`,
          );
          return false;
        }
        return this.finalizeWithBusyRetry()
          .then((result) => {
            recordBoardRevision(boardId, result.revision);
            if (this.boardId !== boardId) return true;
            this.revision = result.revision;
            this.finalized = true;
            diagnosticsLog(
              "info",
              `pin-board finalize done: boardId=${boardId}, revision=${result.revision}`,
            );
            return true;
          });
      })
      .catch((error) => {
        diagnosticsLog(
          "warn",
          `pin-board finalize failed: boardId=${boardId}, error=${errorMessage(error)}`,
        );
        if (this.boardId === boardId) this.onError(`画板结算失败：${errorMessage(error)}`);
        return false;
      })
      .finally(() => {
        if (this.finalizePromise === task) this.finalizePromise = null;
      });
    this.finalizePromise = task;
    return task;
  }

  /**
   * 提交结算。退出与切仓库时自动备份可能正持仓库写锁：后端会让结算插队并等待
   * 最多 10 秒，残留的 `repository_busy` 再重试一次，仍失败才上报。
   */
  private finalizeWithBusyRetry(): Promise<SavePinBoardResult> {
    return pinBoardApi.finalizePinBoard(this.boardId, this.revision)
      .catch(async (error: unknown) => {
        if (!isRepositoryBusy(error)) throw error;
        diagnosticsLog(
          "info",
          `pin-board finalize busy, retrying once: boardId=${this.boardId}`,
        );
        await new Promise((resolve) => setTimeout(resolve, FINALIZE_BUSY_RETRY_DELAY_MS));
        return pinBoardApi.finalizePinBoard(this.boardId, this.revision);
      });
  }

  private stateKey(): string {
    return JSON.stringify(this.images.map(({ itemId, order, layer, deleted, points, uv }) => ({
      itemId,
      order,
      layer,
      deleted,
      points,
      uv,
    })));
  }

  private editableImages(): PinBoardEditableItem[] {
    return this.images.map(({ itemId, order, layer, deleted, points, uv }) => ({
      itemId,
      order,
      layer,
      deleted,
      points: cloneQuad(points),
      uv: cloneQuad(uv),
    }));
  }

  private emitState() {
    if (this.destroyed || !this.loaded) return;
    this.canvas.classList.toggle("locked", this.locked);
    this.updateSelectionOverlay();
    this.persistSession();
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
    if (!(event.ctrlKey || event.metaKey)) {
      if (!event.altKey && !event.shiftKey) {
        const tool = toolForShortcut(event.key);
        if (tool) {
          event.preventDefault();
          this.setTool(tool);
        }
      }
      return;
    }
    const key = event.key.toLowerCase();
    if (key === "s") {
      event.preventDefault();
      this.finishActiveInteraction();
      void this.save();
    } else if (key === "a" && !event.shiftKey) {
      event.preventDefault();
      this.selectAll();
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
    if (this.loaded
      && this.pendingViewportFit
      && cssWidth >= VIEWPORT_FIT_MIN_CANVAS_CSS_PIXELS
      && cssHeight >= VIEWPORT_FIT_MIN_CANVAS_CSS_PIXELS) {
      // 画布尺寸首次可用时执行打开适配；模块隐藏期间创建的 renderer 会在这里补做。
      this.applyContentFit(cssWidth, cssHeight);
    }
    if (this.preservedWorldUnitsPerCssPixel !== null) {
      this.viewport.height = this.clampViewportHeight(
        this.preservedWorldUnitsPerCssPixel * cssHeight,
      );
    } else if (this.loaded && this.cssHeight > 0 && cssHeight !== this.cssHeight) {
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
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
    if (!this.loaded) return;
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
    } else if (isStrokeTool(this.tool)) {
      // 绘制分支与选中/框选互斥：工具不是 select 时，按下只可能是要画一笔。
      if (!this.beginDrawing(this.dragWorld)) return;
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
        // Ctrl/Cmd 与 Shift 都作为加选修饰键：按住点击单张图切换选中，按住在空白处
        // 拖拽则累加框选。
        const additiveSelection = isAdditiveSelectionModifier(event);
        if (additiveSelection && hit) {
          if (this.selectedIds.has(hit.itemId)) this.selectedIds.delete(hit.itemId);
          else {
            this.selectedIds.add(hit.itemId);
            this.bringToFront([hit.itemId]);
          }
          this.refreshSelectionVisuals();
          event.preventDefault();
          return;
        }

        const selection = this.selectionWorldQuad();
        const insideSelection = selection
          ? pointInQuad(this.dragWorld, selection)
          : false;
        if (hit && this.selectedIds.has(hit.itemId)) {
          this.dragMode = "images";
          this.dragBefore = this.snapshot([...this.selectedIds]);
        } else if (hit) {
          this.selectedIds.clear();
          this.selectedIds.add(hit.itemId);
          this.bringToFront([hit.itemId]);
          this.dragMode = "images";
          this.dragBefore = this.snapshot([hit.itemId]);
          this.refreshSelectionVisuals();
        } else if (insideSelection) {
          this.dragMode = "images";
          this.dragBefore = this.snapshot([...this.selectedIds]);
        } else {
          this.marqueeBaseIds.clear();
          if (additiveSelection) {
            // 累加式框选：保留当前选中作为基底，框到的新图在此基础上加入，
            // 因此可以连续多次框选，已选中的不会消失。
            for (const itemId of this.selectedIds) this.marqueeBaseIds.add(itemId);
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
    this.canvas.classList.add(this.dragMode ? DRAG_CURSOR_CLASS[this.dragMode] : "selecting");
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
      if (this.dragMode === "draw") {
        // 只更新草稿与 DOM 预览层：GPU 缓冲在落库后由 `applyMutationResult` 重建，
        // 不在每次 pointermove 重建整板笔迹。
        if (this.draft) {
          extendStrokeDraft(this.draft, world, event.shiftKey);
          this.drawStrokePreview();
        }
      } else if (this.dragMode === "images") {
        const deltaX = world[0] - this.dragWorld[0];
        const deltaY = world[1] - this.dragWorld[1];
        for (const snapshot of this.dragBefore) {
          const image = this.imageById.get(snapshot.itemId);
          if (image) image.points = translateQuad(snapshot.points, deltaX, deltaY);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.itemId));
      } else if (this.dragMode === "rotate") {
        const delta = Math.atan2(
          world[1] - this.rotateCenter[1],
          world[0] - this.rotateCenter[0],
        ) - this.rotateStartAngle;
        for (const snapshot of this.dragBefore) {
          const image = this.imageById.get(snapshot.itemId);
          if (!image) continue;
          // Shift 吸附图片的**绝对角度**（0°/15°/30°…）而非本次拖拽的增量：
          // 增量吸附会让已带偏角的图片停在「偏角 + n×15°」，永远回不到 0°。
          const radians = event.shiftKey ? snapRotationDelta(snapshot.points, delta) : delta;
          image.points = rotateQuad(snapshot.points, radians, this.rotateCenter);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.itemId));
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
          const image = this.imageById.get(snapshot.itemId);
          if (image) image.points = scaleQuad(snapshot.points, factor, this.scaleAnchor);
        }
        this.afterLiveChange(this.dragBefore.map((image) => image.itemId));
      } else {
        this.marqueeWorld = world;
        const bounds = this.boundsBetween(this.dragWorld, world);
        // 累加式框选：以 marqueeBaseIds 为基底并入框到的图片，因此可连续多次框选，
        // 已选中的不会消失，框到的新图加入。隐藏图层先剔掉，否则「框不到却选得上」。
        const next = marqueeSelection(
          this.images.filter((image) => isLayerVisible(this.visibleLayers, image.layer)),
          bounds,
          this.marqueeBaseIds,
        );
        // 框选矩形是独立的 DOM 叠加层，每个指针事件都要跟随；但选中集合未变时
        // （例如拖过空白区域或已选中的图片）无需重排、重绘与广播状态。
        this.updateMarqueeOverlay(bounds);
        if (!sameSelection(this.selectedIds, next)) {
          this.selectedIds.clear();
          for (const itemId of next) this.selectedIds.add(itemId);
          this.refreshSelectionVisuals();
        }
      }
    }
    event.preventDefault();
  };

  private readonly pointerUp = (event: PointerEvent) => {
    if (!this.dragMode || this.dragPointerId !== event.pointerId) return;
    if (this.dragMode === "draw") this.commitDrawing();
    else this.finishInteraction();
    event.preventDefault();
  };

  /**
   * 指针被系统收回（触摸被接管、窗口失焦等）：这一笔按取消处理，
   * 与 `pointerup` 的「落库」区分开——事件已经不可信，不该拿半条轨迹建条目。
   */
  private readonly pointerCancel = (event: PointerEvent) => {
    if (!this.dragMode || this.dragPointerId !== event.pointerId) return;
    if (this.dragMode === "draw") this.cancelDrawing();
    else this.finishInteraction();
    event.preventDefault();
  };

  private finishInteraction() {
    // 绘制的中断路径（锁定、切画板、失焦）一律按取消处理：没有落库就什么也没发生。
    if (this.dragMode === "draw") {
      this.cancelDrawing();
      return;
    }
    const pointerId = this.dragPointerId;
    if ((this.dragMode === "images" || this.dragMode === "rotate" || this.dragMode === "scale")
      && this.dragBefore.length > 0) {
      const after = this.snapshot(this.dragBefore.map((image) => image.itemId));
      this.pushHistory(this.dragBefore, after);
      this.afterModelChange(after.map((image) => image.itemId));
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
      "drawing",
      "over-rotate-handle",
      "over-resize-nwse",
      "over-resize-nesw",
    );
    this.emitState();
  }

  /**
   * 开始绘制。选中态先清掉：绘制中不该同时显示变换手柄；落库后的新条目只在
   * `select` 工具下自动选中（见 `applyMutationResult`），绘制工具下保持干净。
   */
  private beginDrawing(world: Point): boolean {
    if (!this.loaded || !isStrokeTool(this.tool)) return false;
    this.selectedIds.clear();
    this.refreshSelectionVisuals();
    this.draft = beginStrokeDraft(this.tool, world);
    this.dragMode = "draw";
    this.drawStrokePreview();
    return true;
  }

  /**
   * 结束绘制：把草稿编码成落库请求并交给控制器。
   *
   * 预览层**先不清理**：落库要经过保存、创建与重读几何，立刻清掉会闪一下空白；
   * 预览留到 `clearPendingStrokePreview` 被调用（新笔迹进缓冲或提交失败）。
   * 拖拽距离不足（误点）或编码失败时按取消处理，不产生条目。
   */
  private commitDrawing() {
    const draft = this.draft;
    const request = draft
      ? resolveStrokeDraft(
        draft,
        this.strokeWidthValue,
        MIN_STROKE_DRAG_CSS_PIXELS * this.worldUnitsPerCssPixel(),
      )
      : null;
    if (!draft || !request) {
      this.cancelDrawing();
      return;
    }
    this.releaseDrawingPointer();
    this.onStrokeCommit({ ...request, color: this.strokeColor });
  }

  /** 交还指针与光标类，保留草稿（提交路径要留着预览）。 */
  private releaseDrawingPointer() {
    const pointerId = this.dragPointerId;
    this.dragMode = null;
    this.dragPointerId = null;
    this.canvas.classList.remove("drawing");
    if (pointerId !== null && this.canvas.hasPointerCapture(pointerId)) {
      this.canvas.releasePointerCapture(pointerId);
    }
    this.emitState();
  }

  private clearStrokePreview() {
    const overlay = this.drawOverlay;
    const context = overlay?.getContext("2d");
    if (!overlay || !context) return;
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, overlay.width, overlay.height);
  }

  /**
   * 把当前草稿画到 DOM 叠加层上。坐标直接用世界单位：把变换设成「世界 → 设备像素」
   * 之后，线宽与形状尺寸天然与落库后的渲染一致，不必两处各写一份换算。
   */
  private drawStrokePreview() {
    const overlay = this.drawOverlay;
    if (!overlay) return;
    const context = overlay.getContext("2d");
    if (!context) return;
    const rect = this.canvas.getBoundingClientRect();
    const cssWidth = Math.max(rect.width, 1);
    const cssHeight = Math.max(rect.height, 1);
    const ratio = Math.max(1, window.devicePixelRatio || 1);
    const pixelWidth = Math.max(1, Math.round(cssWidth * ratio));
    const pixelHeight = Math.max(1, Math.round(cssHeight * ratio));
    if (overlay.width !== pixelWidth || overlay.height !== pixelHeight) {
      overlay.width = pixelWidth;
      overlay.height = pixelHeight;
    }
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, overlay.width, overlay.height);
    const draft = this.draft;
    if (!draft || !this.loaded) return;
    const bounds = viewportBounds(this.viewport, cssWidth, cssHeight);
    const scale = pixelWidth / Math.max(bounds.maxX - bounds.minX, 1e-9);
    context.setTransform(scale, 0, 0, -scale, -bounds.minX * scale, bounds.maxY * scale);
    context.strokeStyle = this.strokeColor;
    context.fillStyle = this.strokeColor;
    context.lineWidth = this.strokeWidthValue;
    context.lineJoin = "round";
    context.lineCap = "round";
    const points = strokeDraftPoints(draft);
    const [start, end] = points;
    switch (draft.tool) {
      case "pen": {
        if (points.length < 2) return;
        context.beginPath();
        context.moveTo(points[0][0], points[0][1]);
        for (let index = 1; index < points.length; index += 1) {
          context.lineTo(points[index][0], points[index][1]);
        }
        context.stroke();
        return;
      }
      case "line":
        this.strokePreviewSegment(context, start, end);
        return;
      case "arrow": {
        const deltaX = end[0] - start[0];
        const deltaY = end[1] - start[1];
        const length = Math.hypot(deltaX, deltaY);
        if (length < 1e-6) return;
        const unitX = deltaX / length;
        const unitY = deltaY / length;
        const headLength = Math.min(length, Math.max(this.strokeWidthValue * 4, 1e-4));
        const halfBase = headLength * 0.5;
        const base: Point = [end[0] - unitX * headLength, end[1] - unitY * headLength];
        // 杆止于三角形底边，与渲染层一致：圆头藏在箭头里，不会从箭尖后面冒出来。
        this.strokePreviewSegment(context, start, base);
        context.beginPath();
        context.moveTo(end[0], end[1]);
        context.lineTo(base[0] - unitY * halfBase, base[1] + unitX * halfBase);
        context.lineTo(base[0] + unitY * halfBase, base[1] - unitX * halfBase);
        context.closePath();
        context.fill();
        return;
      }
      case "rect": {
        const left = Math.min(start[0], end[0]);
        const top = Math.max(start[1], end[1]);
        context.strokeRect(left, top, Math.abs(end[0] - start[0]), -Math.abs(end[1] - start[1]));
        return;
      }
      default: {
        const centerX = (start[0] + end[0]) / 2;
        const centerY = (start[1] + end[1]) / 2;
        const radiusX = Math.abs(end[0] - start[0]) / 2;
        const radiusY = Math.abs(end[1] - start[1]) / 2;
        if (!(radiusX > 0) || !(radiusY > 0)) return;
        context.beginPath();
        context.ellipse(centerX, centerY, radiusX, radiusY, 0, 0, Math.PI * 2);
        context.stroke();
      }
    }
  }

  private strokePreviewSegment(
    context: CanvasRenderingContext2D,
    start: Point,
    end: Point,
  ) {
    context.beginPath();
    context.moveTo(start[0], start[1]);
    context.lineTo(end[0], end[1]);
    context.stroke();
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
    if (!this.locked && hit && !this.selectedIds.has(hit.itemId)) {
      this.selectedIds.clear();
      this.selectedIds.add(hit.itemId);
      this.bringToFront([hit.itemId]);
      this.refreshSelectionVisuals();
    }
    this.onContextMenu({
      clientX: event.clientX,
      clientY: event.clientY,
      selectedCount: this.selectedImages().length,
      arrangeableCount: this.selectedImages().filter((image) => image.kind === "bitmap").length,
      selectionHasStroke: this.selectedImages().some((image) => image.kind === "stroke"),
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

  /** 画板内全部未删除、且所在图层可见的条目轴对齐最小包围框（旋转条目按外接矩形计算）。 */
  private contentBounds(): Bounds | null {
    return imagesBounds(this.images.filter((image) => (
      !image.deleted && isLayerVisible(this.visibleLayers, image.layer)
    )));
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

  private selectedImages(): PinBoardItem[] {
    // 隐藏层里的条目不算选中：删除、导出、变换、图层切换都从这里取条目。
    return this.images.filter((image) => (
      !image.deleted
      && isLayerVisible(this.visibleLayers, image.layer)
      && this.selectedIds.has(image.itemId)
    ));
  }

  private bringToFront(itemIds: number[]) {
    if (itemIds.length === 0) return;
    const promotedIds = new Set(itemIds);
    const ordered = [...this.images].sort((left, right) => (
      left.order - right.order || left.itemId - right.itemId
    ));
    // 笔迹固定占标注层，**只在笔迹内部**提前：跨层先后关系由 layer 决定，
    // 把笔迹提到整板最前只会白白改写每个条目的 sort_order（一次点击就脏整块画板）。
    const groups = [
      ordered.filter((image) => image.kind !== "stroke"),
      ordered.filter((image) => image.kind === "stroke"),
    ];
    let changed = false;
    for (const group of groups) {
      const promoted = group.filter((image) => promotedIds.has(image.itemId));
      if (promoted.length === 0) continue;
      const rest = group.filter((image) => !promotedIds.has(image.itemId));
      // 只重排本组自己的 order 值（同一个多重集换个顺序），不与另一组抢号，
      // 因此另一组的条目一个字段都不会动。
      const slots = group.map((image) => image.order).sort((left, right) => left - right);
      [...promoted, ...rest].forEach((image, index) => {
        changed ||= image.order !== slots[index];
        image.order = slots[index];
      });
    }
    if (changed) {
      this.dirty = this.stateKey() !== this.savedStateKey;
      if (this.dirty) this.scheduleAutosave();
      this.sortVisibleImages();
      this.draw();
    }
  }

  private hitImage(point: Point): PinBoardItem | null {
    for (let index = this.visibleImages.length - 1; index >= 0; index -= 1) {
      const image = this.visibleImages[index];
      if (pointInQuad(point, image.points)) return image;
    }
    return null;
  }

  private snapshot(itemIds: number[]): ImageSnapshot[] {
    return itemIds.flatMap((itemId) => {
      const image = this.imageById.get(itemId);
      return image ? [{
        itemId,
        deleted: image.deleted,
        layer: image.layer,
        points: cloneQuad(image.points),
        uv: cloneQuad(image.uv),
      }] : [];
    });
  }

  private restore(snapshots: ImageSnapshot[]) {
    for (const snapshot of snapshots) {
      const image = this.imageById.get(snapshot.itemId);
      if (!image) continue;
      image.deleted = snapshot.deleted;
      image.layer = snapshot.layer;
      image.points = cloneQuad(snapshot.points);
      image.uv = cloneQuad(snapshot.uv);
    }
  }

  private commitMutation(
    mutate: (image: PinBoardItem) => void,
    mergeKey?: string,
  ) {
    if (this.locked || this.selectedIds.size === 0) return;
    const itemIds = [...this.selectedIds];
    const before = this.snapshot(itemIds);
    for (const itemId of itemIds) {
      const image = this.imageById.get(itemId);
      if (image) mutate(image);
    }
    const after = this.snapshot(itemIds);
    this.pushHistory(before, after, mergeKey);
    this.afterModelChange(itemIds);
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

  private afterLiveChange(itemIds: number[]) {
    this.dirty = true;
    this.syncGeometryBuffers(itemIds);
    this.refreshViewport(VIEWPORT_LOAD_DELAY_MS);
    this.emitState();
  }

  private afterModelChange(itemIds: number[]) {
    for (const itemId of [...this.selectedIds]) {
      if (this.imageById.get(itemId)?.deleted) this.selectedIds.delete(itemId);
    }
    this.dirty = this.stateKey() !== this.savedStateKey;
    if (this.dirty) this.scheduleAutosave();
    else this.cancelAutosave();
    this.syncGeometryBuffers(itemIds);
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

  private syncGeometryBuffers(itemIds: number[]) {
    let strokeMoved = false;
    for (const itemId of itemIds) {
      const image = this.imageById.get(itemId);
      // 笔迹的几何由整板缓冲承载：变换后让签名失效，下一次 refreshViewport 重建。
      if (image?.kind === "stroke") {
        strokeMoved = true;
        continue;
      }
      const loaded = this.textures.get(itemId) ?? this.previewTextures.get(itemId);
      if (!loaded || !image) continue;
      this.gpu.device.queue.writeBuffer(
        loaded.vertexBuffer,
        0,
        vertexData(image, loaded.uvScale ?? [1, 1]),
      );
      this.gpu.device.queue.writeBuffer(loaded.outlineBuffer, 0, outlineData(image));
    }
    if (strokeMoved) this.strokeSignature = "";
  }

  private refreshViewport(loadDelay: number) {
    if (this.destroyed || this.gpuUnavailable || !this.loaded || this.canvas.width < 1 || this.canvas.height < 1) return;
    this.generation += 1;
    const bounds = viewportBounds(this.viewport, this.canvas.width, this.canvas.height);
    this.visibleImages = this.images.filter((image) => (
      !image.deleted
      && image.available
      && isLayerVisible(this.visibleLayers, image.layer)
      && intersects(imageBounds(image), bounds)
    ));
    this.sortVisibleImages();
    const now = performance.now();
    for (const image of this.visibleImages) {
      const loaded = this.textures.get(image.itemId);
      if (loaded) loaded.lastUsed = now;
      const preview = this.previewTextures.get(image.itemId);
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
        // 笔迹着色器的抗锯齿羽化按「设备像素 / 世界单位」换算。
        this.devicePixelsPerWorldUnit(),
        0,
        0,
        0,
      ]),
    );
    this.syncStrokeBuffers();
    this.evictTextures();
    this.draw();
    // 视口变化时预览层要跟着重算（缩放、窗口尺寸变化都走这里）。
    if (this.draft) this.drawStrokePreview();
    this.scheduleLoads(loadDelay);
  }

  private draw() {
    if (this.destroyed || this.gpuUnavailable) {
      this.clearPlaceholderOverlay();
      return;
    }
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
        const loaded = this.textures.get(image.itemId) ?? this.previewTextures.get(image.itemId);
        if (!loaded) continue;
        pass.setBindGroup(1, loaded.bindGroup);
        pass.setVertexBuffer(0, loaded.vertexBuffer);
        pass.draw(6);
      }
      // 第二趟：笔迹固定在最上层，整板合并成一次 indexed draw，不与图片交错。
      if (this.strokeIndexCount > 0 && this.strokeVertexBuffer && this.strokeIndexBuffer) {
        pass.setPipeline(this.gpu.strokePipeline);
        pass.setBindGroup(0, this.gpu.viewportBindGroup);
        pass.setVertexBuffer(0, this.strokeVertexBuffer);
        pass.setIndexBuffer(this.strokeIndexBuffer, "uint32");
        pass.drawIndexed(this.strokeIndexCount);
      }
      if (this.selectedIds.size > 0) {
        pass.setPipeline(this.gpu.outlinePipeline);
        pass.setBindGroup(0, this.gpu.viewportBindGroup);
        for (const image of this.visibleImages) {
          if (!this.selectedIds.has(image.itemId)) continue;
          if (image.kind === "stroke") {
            const slot = this.strokeOutlineIndex.get(image.itemId);
            if (slot === undefined || !this.strokeOutlineBuffer) continue;
            pass.setVertexBuffer(0, this.strokeOutlineBuffer);
            pass.draw(STROKE_OUTLINE_VERTICES, 1, slot * STROKE_OUTLINE_VERTICES);
            continue;
          }
          const loaded = this.textures.get(image.itemId) ?? this.previewTextures.get(image.itemId);
          if (!loaded) continue;
          pass.setVertexBuffer(0, loaded.outlineBuffer);
          pass.draw(5);
        }
      }
      pass.end();
      this.gpu.device.queue.submit([encoder.finish()]);
      this.drawErrorReported = false;
      this.updatePlaceholderOverlay();
    } catch (error) {
      if (!this.destroyed && !this.drawErrorReported) {
        this.drawErrorReported = true;
        this.onError(`画布渲染失败：${errorMessage(error)}`);
      }
    }
  }

  /**
   * 同步「纹理未就绪」占位层：为可见但尚无纹理（缩略图或高清）的图片铺一块
   * 灰底方框标示图片范围，中心配一个动态加载图标。加载失败的图片改显静态
   * 告警图标，避免永久转圈。节点按 itemId 复用，只在集合变化时增删。
   */
  private updatePlaceholderOverlay() {
    const layer = this.placeholderElement;
    if (!layer || this.destroyed || this.gpuUnavailable || !this.loaded) {
      this.clearPlaceholderOverlay();
      return;
    }
    const rect = this.canvas.getBoundingClientRect();
    if (rect.width < 1 || rect.height < 1) {
      this.clearPlaceholderOverlay();
      return;
    }
    const pending = new Map<number, { quad: Quad; failed: boolean }>();
    for (const image of this.visibleImages) {
      // 笔迹永远不需要纹理，不该出现「正在加载」或「加载失败」占位框。
      if (image.kind === "stroke") continue;
      if (this.textures.has(image.itemId) || this.previewTextures.has(image.itemId)) continue;
      pending.set(image.itemId, {
        quad: image.points,
        failed: this.failedTextureDimensions.has(image.itemId),
      });
    }
    for (const [itemId, node] of this.placeholderNodes) {
      if (pending.has(itemId)) continue;
      node.remove();
      this.placeholderNodes.delete(itemId);
    }
    for (const [itemId, entry] of pending) {
      let node = this.placeholderNodes.get(itemId);
      if (!node) {
        node = document.createElement("div");
        node.className = "pin-board-placeholder";
        const spinner = document.createElement("span");
        spinner.className = "pin-board-placeholder-spinner";
        const failed = document.createElement("span");
        failed.className = "pin-board-placeholder-failed";
        failed.textContent = "!";
        node.append(spinner, failed);
        layer.appendChild(node);
        this.placeholderNodes.set(itemId, node);
      }
      node.classList.toggle("failed", entry.failed);
      const screenQuad = entry.quad.map((point) => worldToScreen(point, rect, this.viewport)) as Quad;
      const width = Math.max(
        Math.hypot(screenQuad[1][0] - screenQuad[0][0], screenQuad[1][1] - screenQuad[0][1]),
        1,
      );
      const height = Math.max(
        Math.hypot(screenQuad[3][0] - screenQuad[0][0], screenQuad[3][1] - screenQuad[0][1]),
        1,
      );
      const angle = Math.atan2(screenQuad[1][1] - screenQuad[0][1], screenQuad[1][0] - screenQuad[0][0]);
      node.style.transformOrigin = "0 0";
      node.style.transform = `translate(${screenQuad[0][0]}px, ${screenQuad[0][1]}px) rotate(${angle}rad)`;
      node.style.width = `${width}px`;
      node.style.height = `${height}px`;
      // 图标不随图片旋转：外层旋转由内层反向抵消；尺寸随画面缩放，过小的方框只留底色。
      const minSide = Math.min(width, height);
      const icon = Math.max(12, Math.min(30, minSide * 0.26));
      node.style.setProperty("--pb-placeholder-icon", `${icon}px`);
      node.style.setProperty("--pb-placeholder-angle", `${-angle}rad`);
      node.classList.toggle("compact", minSide < 18);
    }
  }

  private clearPlaceholderOverlay() {
    if (!this.placeholderNodes) return;
    for (const node of this.placeholderNodes.values()) node.remove();
    this.placeholderNodes.clear();
  }

  private sortVisibleImages() {
    this.visibleImages.sort((left, right) => (
      left.layer - right.layer
      || right.order - left.order
    ));
  }

  private scheduleLoads(delay: number) {
    if (!this.active || this.destroyed || this.gpuUnavailable || !this.loaded) return;
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
        const candidates = this.textureLoadCandidates();
        // 只有非空批次会记录：空批次会立即结束循环，不构成自旋。
        if (candidates.length > 0) {
          diagnosticsLog(
            "info",
            `pin-board texture drain start: boardId=${this.boardId}, candidates=${candidates.length}, generation=${generation}`,
          );
        }
        const batchStarted = performance.now();
        await this.loadCandidates(candidates, generation);
        // 常速时不产生日志；一旦某批明显变慢即点名（显存压力 / 加载退化的前兆）。
        diagnosticsSlowStep(
          `texture drain batch boardId=${this.boardId} candidates=${candidates.length}`,
          batchStarted,
          2_000,
        );
        if (candidates.length > 0) {
          diagnosticsLog(
            "info",
            `pin-board texture drain done: boardId=${this.boardId}, candidates=${candidates.length},`
            + ` textures=${this.textures.size}, previews=${this.previewTextures.size},`
            + ` reservedBytes=${this.textureReservedBytes}, generation=${this.generation}`,
          );
        }
      }
    } finally {
      this.loading = false;
      if (this.loadRequested && this.active && !this.destroyed) void this.drainLoads();
    }
  }

  private textureLoadCandidates(): TextureLoadCandidate[] {
    const candidates = this.visibleImages.flatMap((image) => {
      // 笔迹没有纹理：进队列会去读 DDS 并失败，最终变成红色失败占位框。
      if (image.kind === "stroke") return [];
      const center = quadCenter(image.points);
      const [width, height] = imageSize(image);
      const distanceSquared = (center[0] - this.viewport.centerX) ** 2
        + (center[1] - this.viewport.centerY) ** 2;
      const priority = distanceSquared / Math.max(width * height, 1);
      const list: TextureLoadCandidate[] = [];
      // 常驻缩略图兜底：没有缩略图时先生成缩略图候选，保证滑动回来后
      // 即使高清纹理尚未加载也能立即绘制，不闪烁。缩略图失败按维度记录，
      // 不会阻止更高分辨率的重试。
      if (!this.previewTextures.has(image.itemId)
        && !this.failedAtOrAbove(image.itemId, PREVIEW_TEXTURE_DIMENSION)) {
        list.push({
          image,
          maxDimension: PREVIEW_TEXTURE_DIMENSION,
          estimatedBytes: estimatedLoadBytes(image, PREVIEW_TEXTURE_DIMENSION),
          priority,
          isPreview: true,
        });
      }
      const desiredDimension = this.requestedDimension(image);
      const qualityCap = this.textureQualityCaps.get(image.itemId);
      const maxDimension = qualityCap
        ? Math.min(desiredDimension, qualityCap)
        : desiredDimension;
      const current = this.textures.get(image.itemId);
      if (!(current && current.maxDimension >= maxDimension * 0.9)
        && !this.failedAtOrAbove(image.itemId, maxDimension)) {
        list.push({
          image,
          maxDimension,
          estimatedBytes: estimatedLoadBytes(image, maxDimension),
          priority,
        });
      }
      return list;
    });
    return candidates.sort((left, right) => (
      Number(this.selectedIds.has(right.image.itemId))
        - Number(this.selectedIds.has(left.image.itemId))
      // 缩略图优先于高清，先给可见图片铺上兜底内容，再逐张清晰化。
      || Number(right.isPreview === true) - Number(left.isPreview === true)
      || left.priority - right.priority
      || right.maxDimension - left.maxDimension
    ));
  }

  /** 该图片在目标尺寸（及其之上）是否已记录过加载失败。 */
  private failedAtOrAbove(itemId: number, maxDimension: number): boolean {
    const failed = this.failedTextureDimensions.get(itemId);
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
          this.inFlightTextureLoads += 1;
          void this.loadCandidate(candidate, generation).finally(() => {
            this.inFlightTextureLoads = Math.max(0, this.inFlightTextureLoads - 1);
            if (this.destroyed) {
              diagnosticsLog(
                "info",
                `texture load settled after renderer destroy: boardId=${this.boardId}, gpuSeq=${this.gpu.sequence}, itemId=${candidate.image.itemId}, inFlightTextureLoads=${this.inFlightTextureLoads}`,
              );
            }
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
    let current: LoadedTexture | null = target.get(image.itemId) ?? null;
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
        const residentBytes = estimatedResidentBytes(image, dimension);
        const reservationBytes = residentBytes + PIN_BOARD_TEXTURE_STAGING_MARGIN_BYTES;
        current = target.get(image.itemId) ?? null;
        const reserved = this.reserveTextureBytes(image.itemId, reservationBytes, isPreview);
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
      if (!isPreview && !this.visibleImages.some((visible) => visible.itemId === image.itemId)) {
        destroyTexture(loaded);
        return;
      }
      const replacement = target.get(image.itemId);
      if (replacement && replacement.maxDimension >= maxDimension * 0.9) {
        destroyTexture(loaded);
        return;
      }
      if (replacement) destroyTexture(replacement);
      target.set(image.itemId, loaded);
      if (!isPreview) {
        if (loaded.maxDimension < maxDimension) {
          this.textureQualityCaps.set(image.itemId, loaded.maxDimension);
        } else {
          this.textureQualityCaps.delete(image.itemId);
        }
      }
      this.evictTextures(image.itemId, 0, isPreview);
      this.failedTextureDimensions.delete(image.itemId);
      this.draw();
    } catch (error) {
      if (this.destroyed || generation !== this.generation) return;
      const previous = this.failedTextureDimensions.get(image.itemId) ?? 0;
      this.failedTextureDimensions.set(image.itemId, Math.max(previous, maxDimension));
      // 缩略图失败静默（高清失败仍按原样上报），避免预算波动反复打扰用户。
      if (!isPreview) this.onError(`图片 ${image.itemId} 加载失败：${errorMessage(error)}`);
    }
  }

  private requestedDimension(image: PinBoardItem): number {
    // 笔迹不建纹理，任何纹理尺寸对它都没有意义。
    if (image.kind === "stroke") return 0;
    const displayDimension = imageDisplayMaxDimension(image, this.viewport, this.canvas.height);
    const deviceLimit = this.gpu.device.limits.maxTextureDimension2D as number;
    if (image.kind === "text") {
      return requestedTextTextureDimension(image, displayDimension, deviceLimit);
    }
    return requestedTextureDimension(image, displayDimension, deviceLimit);
  }

  private evictTextures(protectedImageId?: number, additionalBytes = 0, isPreview = false) {
    const target = isPreview ? this.previewTextures : this.textures;
    const budget = isPreview ? this.previewTextureBudgetBytes : this.textureBudgetBytes;
    let total = [...target.values()].reduce((sum, loaded) => sum + loaded.byteSize, 0);
    if (total + additionalBytes <= budget) return;
    const visible = new Set(this.visibleImages.map((image) => image.itemId));
    const candidates = [...target.entries()]
      .filter(([itemId]) => itemId !== protectedImageId)
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
    for (const [itemId, loaded] of candidates) {
      destroyTexture(loaded);
      target.delete(itemId);
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
