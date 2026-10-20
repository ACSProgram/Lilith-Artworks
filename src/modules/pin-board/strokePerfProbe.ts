/**
 * 开发期自检：生成 N 笔合成笔迹，量一遍「采样管道 + 编码」与「整板缓冲构建」的耗时。
 *
 * 只在开发构建里挂到 `globalThis`（`installStrokePerfProbe`），供维护者在 DevTools
 * 控制台复测：
 *
 * ```js
 * __lilithPinBoardStrokeProbe({ strokes: 500, pointsPerStroke: 300 })
 * ```
 *
 * 它**不写仓库、不碰 GPU**：合成数据只在内存里走一遍真实编码与缓冲构建路径，因此
 * 能量到 CPU 侧的成本，量不到帧率。GPU 帧率只能在真实适配器上手工看（见
 * `docs/modules/pin-board.md` 的「快速验证」）。
 */

import type { Point, Quad } from "./geometry";
import { STROKE_TOOL, decodeStrokeHeader } from "./strokeGeometry";
import {
  beginStrokeDraft,
  extendStrokeDraft,
  resolveStrokeDraft,
  type StrokeToolName,
} from "./strokeDrawing";
import {
  STROKE_VERTEX_FLOATS,
  buildStrokeBatch,
  type StrokeDrawEntry,
} from "./strokeRender";
import { normalisePenSamples, type PressureSample } from "./strokeSampling";
import type { PinBoardStrokeItem } from "./types";

export interface StrokePerfProbeOptions {
  /** 笔数，默认 500。 */
  strokes?: number;
  /** 每笔的原始采样点数，默认 300（约 5 秒的 60 Hz 拖拽）。 */
  pointsPerStroke?: number;
  /** 绘制时的缩放（设备像素 / 世界单位），默认 1。 */
  pixelsPerWorldUnit?: number;
  strokeWidth?: number;
}

export interface StrokePerfProbeResult {
  strokes: number;
  pointsPerStroke: number;
  /** 编码前的采样点总数。 */
  sampledPoints: number;
  /** 采样管道之后的点总数（写入 BLOB 的点）。 */
  encodedPoints: number;
  /** 几何 BLOB 的总字节数。 */
  geometryBytes: number;
  /** 只跑采样管道（抽稀、光滑、重采样、压感）的耗时。 */
  samplingMs: number;
  /** 草稿构造 + 采样管道 + 编码的总耗时（含 `samplingMs`）。 */
  encodeMs: number;
  /** 整板缓冲构建（顶点/索引 + 逐段裁剪）的耗时。 */
  buildMs: number;
  primitives: number;
  vertices: number;
  /** 每笔的平均编码耗时。 */
  encodePerStrokeMs: number;
  /** 每笔的平均缓冲构建耗时。 */
  buildPerStrokeMs: number;
}

/**
 * 生成一条确定性的合成笔迹：正弦中心线 + 起伏压感。`index` 决定起点与相位，
 * 因此多笔之间不会完全重合。
 */
export function syntheticStroke(
  index: number,
  pointsPerStroke: number,
): PressureSample[] {
  const count = Math.max(2, Math.floor(pointsPerStroke));
  return Array.from({ length: count }, (_, step) => {
    const t = step / (count - 1);
    const phase = index * 0.7;
    return {
      point: [index * 40 + t * 400, Math.sin(t * Math.PI * 2 + phase) * 80] as Point,
      pressure: 0.35 + 0.5 * Math.abs(Math.sin(t * Math.PI * 3 + phase)),
    };
  });
}

/** 走一遍真实的草稿 → 编码路径，返回落库请求与条目。 */
function encodeStroke(
  samples: readonly PressureSample[],
  strokeWidth: number,
  pixelsPerWorldUnit: number,
): StrokeDrawEntry | null {
  const draft = beginStrokeDraft("pen" as StrokeToolName, samples[0].point, {
    pressure: samples[0].pressure,
    pointerType: "pen",
  });
  for (const sample of samples.slice(1)) {
    extendStrokeDraft(draft, sample.point, false, {
      pressure: sample.pressure,
      pointerType: "pen",
    });
  }
  const commit = resolveStrokeDraft(draft, strokeWidth, 0, pixelsPerWorldUnit);
  if (!commit) return null;
  const left = commit.centerX - commit.width / 2;
  const top = commit.centerY + commit.height / 2;
  const points: Quad = [
    [left, top],
    [left + commit.width, top],
    [left + commit.width, top - commit.height],
    [left, top - commit.height],
  ];
  const item: PinBoardStrokeItem = {
    boardId: 0,
    itemId: 0,
    kind: "stroke",
    width: commit.width,
    height: commit.height,
    order: 0,
    layer: 3,
    deleted: false,
    points,
    uv: [[0, 0], [1, 0], [1, 1], [0, 1]],
    available: true,
    tool: STROKE_TOOL.pen,
    color: "#e5484d",
    strokeWidth: commit.strokeWidth,
  };
  return {
    item,
    geometry: {
      ...decodeStrokeHeader(commit.geometry),
      itemId: 0,
      color: item.color,
      strokeWidth: commit.strokeWidth,
      width: commit.width,
      height: commit.height,
      geometry: commit.geometry,
    },
  };
}

/** 跑一次自检并把结果打点。纯计算，可安全地在控制台反复调用。 */
export function runStrokePerfProbe(
  options: StrokePerfProbeOptions = {},
): StrokePerfProbeResult {
  const strokes = Math.max(1, Math.floor(options.strokes ?? 500));
  const pointsPerStroke = Math.max(2, Math.floor(options.pointsPerStroke ?? 300));
  const pixelsPerWorldUnit = Math.max(options.pixelsPerWorldUnit ?? 1, 1e-6);
  const strokeWidth = Math.max(options.strokeWidth ?? 6, 0.01);
  // 合成数据只生成一次，三个阶段测的都是同一批采样。
  const samples = Array.from({ length: strokes }, (_, index) => (
    syntheticStroke(index, pointsPerStroke)
  ));
  const sampledPoints = samples.reduce((total, entry) => total + entry.length, 0);

  const samplingStarted = performance.now();
  let encodedPoints = 0;
  for (const entry of samples) {
    encodedPoints += normalisePenSamples(entry, {
      pixelsPerWorldUnit,
      strokeWidth,
      usePressure: true,
    }).length;
  }
  const samplingMs = performance.now() - samplingStarted;

  const encodeStarted = performance.now();
  const entries: StrokeDrawEntry[] = [];
  let geometryBytes = 0;
  for (let index = 0; index < samples.length; index += 1) {
    const entry = encodeStroke(samples[index], strokeWidth, pixelsPerWorldUnit);
    if (!entry) continue;
    entry.item.itemId = index + 1;
    entry.geometry.itemId = index + 1;
    geometryBytes += entry.geometry.geometry.byteLength;
    entries.push(entry);
  }
  const encodeMs = performance.now() - encodeStarted;

  const buildStarted = performance.now();
  const batch = buildStrokeBatch(entries, { pixelsPerWorldUnit });
  const buildMs = performance.now() - buildStarted;

  return {
    strokes,
    pointsPerStroke,
    sampledPoints,
    encodedPoints,
    geometryBytes,
    samplingMs,
    encodeMs,
    buildMs,
    primitives: batch.primitiveCount,
    vertices: batch.vertices.length / STROKE_VERTEX_FLOATS,
    encodePerStrokeMs: encodeMs / strokes,
    buildPerStrokeMs: buildMs / strokes,
  };
}

/** 把自检入口挂到全局；只应在开发构建里调用。 */
export function installStrokePerfProbe(): void {
  const target = globalThis as unknown as Record<string, unknown>;
  target.__lilithPinBoardStrokeProbe = (options?: StrokePerfProbeOptions) => {
    const result = runStrokePerfProbe(options);
    if (typeof console !== "undefined" && typeof console.table === "function") {
      console.table(result);
    }
    return result;
  };
}
