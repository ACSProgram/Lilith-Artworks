/**
 * 素材板渲染器的 GPU 层：设备/上下文/管线创建、纹理上传与销毁、DDS 与 RGBA 负载校验、
 * 以及导出回读共用的 GPU 常量。
 *
 * 这一层不知道画板、图层的存在：入口只接受 `PinBoardItem` 与目标长边，出口是可直接
 * 绑进渲染 pass 的纹理与顶点/描边缓冲。设备丢失后的重建也复用 `makeGpu`，避免两条
 * 创建路径分叉。
 */

import { diagnosticsLog } from "../../shared/diagnostics";
import { pinBoardApi } from "./api";
import type { PinBoardItem } from "./types";
import {
  PIN_BOARD_NATIVE_TEXTURE_LIMIT,
  PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES,
  PIN_BOARD_TEXTURE_UPLOAD_BUDGET_BYTES,
  bc7TextureDimensions,
  bc7TextureUvScale,
  estimatedTextureLoadBytes,
  estimatedTextureResidentBytes,
  textureUploadLayout,
} from "./texturePolicy";
import { textRgbaPayload, textTextureResidentBytes, textTextureRgbaBytes } from "./textLayout";
import { STROKE_VERTEX_BYTES } from "./strokeRender";

export type GpuState = {
  canvas: HTMLCanvasElement;
  device: any;
  context: any;
  pipeline: any;
  outlinePipeline: any;
  /** 笔迹 SDF 管线：整板笔迹合并成一条三角形列表一次画完。 */
  strokePipeline: any;
  /**
   * 整画布导出的离屏管线：与屏幕那三条只有目标格式不同（`rgba8unorm` 而不是画布
   * 格式），着色器与绑定布局完全共用。回读后能直接按 RGBA 编 PNG，省一次通道交换。
   */
  exportPipeline: any;
  exportStrokePipeline: any;
  /** 导出专用 uniform（逐块写入），与屏幕的 `viewportBuffer` 分开。 */
  exportViewportBuffer: any;
  exportViewportBindGroup: any;
  sampler: any;
  viewportBuffer: any;
  viewportBindGroup: any;
  /** 诊断用：本设备的创建序号，便于在日志里与销毁记录配对。 */
  sequence: number;
  /** 设备释放后，异步纹理读取返回时不得再次触碰 WebGPU 对象。 */
  released: boolean;
};

export type LoadedTexture = {
  texture: any;
  bindGroup: any;
  vertexBuffer: any;
  outlineBuffer: any;
  byteSize: number;
  maxDimension: number;
  uvScale: [number, number];
  lastUsed: number;
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
    // 椭圆：用「隐函数值 / 梯度长度」近似距离。早先用 min(radii) 缩放隐函数，长短轴
    // 差距越大越低估距离，笔画的左右两端会明显变粗并被四边形切平；这个近似在常见的
    // 长短轴比例下误差只有几个百分点，扁椭圆不会在两端变粗（极端细长的仍有偏差）。
    let radii = max(input.anchorB, vec2f(1e-4));
    let p = input.local - input.anchorA;
    let k0 = length(p / radii);
    let k1 = length(p / (radii * radii));
    let approx = k0 * (k0 - 1.0) / max(k1, 1e-8);
    dist = abs(approx) - input.radius.x;
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

/** 离屏导出目标的格式：回读后直接就是 PNG 需要的 RGBA 字节序。 */
export const EXPORT_TARGET_FORMAT = "rgba8unorm";
/** GPU 纹理用途位：RENDER_ATTACHMENT / COPY_SRC / COPY_DST / MAP_READ。 */
export const TEXTURE_USAGE_RENDER_ATTACHMENT = 0x10;
export const TEXTURE_USAGE_COPY_SRC = 0x01;
export const BUFFER_USAGE_MAP_READ = 0x01;
export const BUFFER_USAGE_COPY_DST = 0x08;
/** `GPUMapMode.READ`。 */
export const MAP_MODE_READ = 0x01;
/** 回读时 `bytesPerRow` 必须是 256 的倍数。 */
export const READBACK_ROW_ALIGNMENT = 256;

/**
 * 按目标格式建渲染管线。屏幕（画布首选格式）与整画布导出（`rgba8unorm`）共用这一份：
 * 着色器与绑定布局完全相同，只有 `format` 不同，因此离屏导出不需要第二套着色器。
 */
function createBoardPipelines(
  device: any,
  module: any,
  strokeModule: any,
  viewportLayout: any,
  textureLayout: any,
  format: string,
  includeOutline: boolean,
) {
  const blend = {
    color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
    alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
  };
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
      targets: [{ format, blend }],
    },
    primitive: { topology: "triangle-list" },
  });
  const outlinePipeline = includeOutline
    ? device.createRenderPipeline({
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
    })
    : null;
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
      targets: [{ format, blend }],
    },
    primitive: { topology: "triangle-list" },
  });
  return { pipeline, outlinePipeline, strokePipeline };
}

const QUAD_TRIANGLE_ORDER = [0, 1, 3, 3, 1, 2] as const;

export function vertexData(
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

export function outlineData(image: Pick<PinBoardItem, "points">): Float32Array {
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

/** 渲染器创建/销毁时维护存活计数；返回值用于写进生命周期日志。 */
export function adjustLiveRenderers(delta: number): number {
  liveRenderers = Math.max(0, liveRenderers + delta);
  return liveRenderers;
}

/**
 * 解绑画布上下文并丢弃当前 owner 记录。旧设备已失效时忽略 `unconfigure` 的失败；
 * 若 canvas 已被新渲染器接管，则只记一条日志，不去碰新设备的 surface。
 */
export function releaseCanvasGpu(gpu: GpuState) {
  if (canvasGpuOwners.get(gpu.canvas) === gpu.sequence) {
    try {
      gpu.context.unconfigure();
    } catch {
      // 上下文已失效时忽略。
    }
    canvasGpuOwners.delete(gpu.canvas);
    return;
  }
  diagnosticsLog(
    "info",
    `gpu unconfigure skipped for stale owner: gpuSeq=${gpu.sequence}, currentOwner=${canvasGpuOwners.get(gpu.canvas) ?? "none"}`,
  );
}

export async function makeGpu(canvas: HTMLCanvasElement): Promise<GpuState> {
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
  const strokeModule = device.createShaderModule({ code: STROKE_SHADER });
  const { pipeline, outlinePipeline, strokePipeline } = createBoardPipelines(
    device,
    module,
    strokeModule,
    viewportLayout,
    textureLayout,
    format,
    true,
  );
  // 离屏导出复用同一份着色器与绑定布局，只把目标格式换成 rgba8unorm；导出不画选中
  // 描边，因此不需要那条管线。
  const { pipeline: exportPipeline, strokePipeline: exportStrokePipeline } = createBoardPipelines(
    device,
    module,
    strokeModule,
    viewportLayout,
    textureLayout,
    EXPORT_TARGET_FORMAT,
    false,
  );
  const sampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
  const viewportBuffer = device.createBuffer({
    size: VIEWPORT_UNIFORM_BYTES,
    usage: 0x40 | 0x08,
  });
  const viewportBindGroup = device.createBindGroup({
    layout: viewportLayout,
    entries: [{ binding: 0, resource: { buffer: viewportBuffer } }],
  });
  const exportViewportBuffer = device.createBuffer({
    size: VIEWPORT_UNIFORM_BYTES,
    usage: 0x40 | 0x08,
  });
  const exportViewportBindGroup = device.createBindGroup({
    layout: viewportLayout,
    entries: [{ binding: 0, resource: { buffer: exportViewportBuffer } }],
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
    exportPipeline,
    exportStrokePipeline,
    exportViewportBuffer,
    exportViewportBindGroup,
    sampler,
    viewportBuffer,
    viewportBindGroup,
    sequence: gpuDevicesCreated,
    released: false,
  };
}

/** RGBA 负载的魔数判定：至少要有 12 字节头部（魔数 + 宽 + 高）才算数。 */
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

export function estimatedLoadBytes(image: PinBoardItem, maxDimension: number): number {
  if (image.kind === "text") {
    // 文字没有 DDS：负载是前端栅格化出的 RGBA 加头，另外留一份给 canvas 到
    // 上传缓冲的临时拷贝。
    return 12 + textTextureRgbaBytes(image, maxDimension) * 2;
  }
  return estimatedTextureLoadBytes(image, maxDimension);
}

export function estimatedResidentBytes(image: PinBoardItem, maxDimension: number): number {
  if (image.kind === "text") return textTextureResidentBytes(image, maxDimension);
  // 笔迹是 SDF 几何，不占纹理预算。
  if (image.kind === "stroke") return 0;
  return estimatedTextureResidentBytes(image, maxDimension);
}

export async function loadTexture(
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

export function destroyTexture(loaded: LoadedTexture) {
  loaded.texture.destroy();
  loaded.vertexBuffer.destroy();
  loaded.outlineBuffer.destroy();
}

/** 画布背景色：从 CSS 背景解析，拿不到时回落到深色。 */
export function clearColor(canvas: HTMLCanvasElement) {
  const values = getComputedStyle(canvas).backgroundColor.match(/[\d.]+/g)?.map(Number);
  if (!values || values.length < 3) return { r: 0.08, g: 0.1, b: 0.09, a: 1 };
  return {
    r: values[0] / 255,
    g: values[1] / 255,
    b: values[2] / 255,
    a: values.length > 3 ? values[3] : 1,
  };
}
