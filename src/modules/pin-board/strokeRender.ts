/**
 * 笔迹渲染的纯几何层：把 `read_pin_board_strokes` 的几何 BLOB 转成一条可以一次性
 * 提交的 SDF 顶点/索引缓冲。
 *
 * 笔迹不建纹理：每个图元（线段、矩形、椭圆、箭头三角）都是一个贴着图元外框的四边形，
 * 片元着色器在**局部坐标**里算距离场并做 1 px 羽化，因此缩放 10%–800% 时边缘始终
 * 清晰，也不会像「按包围盒开 RGBA 纹理」那样为细长笔迹浪费显存。
 *
 * 坐标约定：
 *
 * - 局部坐标以条目包围盒为界，`x ∈ [0, width]`、`y ∈ [0, height]`，原点在包围盒
 *   左上角（与图片 UV 同一取向，`v = 0` 在 `points[0]` 一侧）；
 * - 局部坐标经条目四角的双线性映射落到世界空间，因此移动、旋转、缩放**不需要**
 *   任何额外变换代码；`uv` 被翻转时（左右/上下镜像）同一份映射会跟着镜像；
 * - 包围盒是**墨迹外框**：矩形与椭圆的路径内缩半个线宽，直线与箭头的两端点也内缩
 *   半个线宽。写入侧必须按线宽把包围盒向外膨胀（见计划的「风险 4」），读侧据此还原。
 */

import {
  MAX_STROKE_POINTS,
  STROKE_GEOMETRY_VERSION,
  STROKE_HEADER_BYTES,
  STROKE_POINT_BYTES,
  STROKE_TOOL,
  type PinBoardStrokeGeometry,
  type PinBoardStrokeTool,
} from "./strokeGeometry";
import type { Bounds, Point, Quad } from "./geometry";
import { quadIntersectsBounds } from "./geometry";
import type { PinBoardStrokeItem } from "./types";

/** 图元类型，与着色器里的 `params.x` 一一对应。 */
export const STROKE_PRIMITIVE_KIND = {
  segment: 0,
  box: 1,
  ellipse: 2,
  triangle: 3,
} as const;

/**
 * 顶点布局：position(2) + local(2) + a(2) + b(2) + c(2) + radius(2) + color(4) + params(2)。
 * 每个图元是一个覆盖墨迹的四边形，4 个顶点 + 6 个索引；选中描边是 5 个顶点的 line-strip。
 */
export const STROKE_VERTEX_FLOATS = 18;
export const STROKE_VERTEX_BYTES = STROKE_VERTEX_FLOATS * 4;
export const STROKE_OUTLINE_VERTICES = 5;
/** 抗锯齿外扩的最小世界尺寸，避免极小的 `pixelsPerWorldUnit` 把外扩算成 0。 */
const STROKE_MIN_AA_WORLD_UNITS = 1e-4;
/** 亚像素点合并的下限：比这更近的点在几何上已经没有意义。 */
const MIN_POINT_DISTANCE = 1e-6;

/** 一笔上的采样点（归一化到条目包围盒）。 */
export interface StrokeSample {
  u: number;
  v: number;
  pressure: number;
}

/** 渲染层的一个图元；坐标都在条目局部空间。 */
interface StrokePrimitive {
  kind: number;
  /** 覆盖图元墨迹的四边形，循环顺序，局部坐标。 */
  quad: Quad;
  a: Point;
  b: Point;
  c: Point;
  radius: [number, number];
}

export interface StrokeDrawEntry {
  item: PinBoardStrokeItem;
  geometry: PinBoardStrokeGeometry;
}

export interface StrokeBatchOptions {
  /** 设备像素 / 世界单位：决定抗锯齿羽化宽度与亚像素点的合并阈值。 */
  pixelsPerWorldUnit: number;
  /**
   * 逐段裁剪的世界包围框（视口 + 超扫边距）。给出时，完全落在框外的图元不写入缓冲：
   * 长笔迹只画看得见的那一段。调用方必须传**扩过边距**的框，否则平移会把还没重建
   * 的段露出来。
   */
  cullBounds?: Bounds;
}

export interface StrokeBatch {
  vertices: Float32Array;
  indices: Uint32Array;
  primitiveCount: number;
  strokeCount: number;
}

/** 解析自由笔的点数据；形状没有点数据，返回空数组。 */
export function decodeStrokeSamples(geometry: PinBoardStrokeGeometry): StrokeSample[] {
  if (geometry.isShape || geometry.pointCount === 0) return [];
  const bytes = geometry.geometry;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const samples: StrokeSample[] = [];
  for (let index = 0; index < geometry.pointCount; index += 1) {
    const offset = STROKE_HEADER_BYTES + index * STROKE_POINT_BYTES;
    samples.push({
      u: view.getUint16(offset, true) / 0xffff,
      v: view.getUint16(offset + 2, true) / 0xffff,
      pressure: bytes[offset + 4] / 0xff,
    });
  }
  return samples;
}

/**
 * 编码几何 BLOB（写入侧）。自由笔逐点量化 `u16 / u16 / u8`；形状只写头部，两端点由
 * 包围盒与方向位推出。超出点数上限直接拒绝——抽稀与重采样由 `strokeSampling.ts`
 * 在编码前完成。
 */
export function encodeStrokeGeometry(
  tool: PinBoardStrokeTool,
  samples: readonly { u: number; v: number; pressure?: number }[] = [],
  direction = 0,
): Uint8Array {
  const isShape = tool !== STROKE_TOOL.pen;
  const directionBits = Math.max(0, Math.min(3, Math.trunc(direction))) & 0b11;
  if (isShape) {
    if (samples.length > 0) throw new Error("形状笔迹不应包含点数据");
    return new Uint8Array([
      STROKE_GEOMETRY_VERSION,
      tool,
      directionBits,
      0,
      0, 0, 0, 0,
    ]);
  }
  if (samples.length === 0) throw new Error("笔迹至少需要一个点");
  if (samples.length > MAX_STROKE_POINTS) throw new Error("笔迹点数超过上限");
  const hasPressure = samples.some((sample) => (sample.pressure ?? 0) > 0);
  const bytes = new Uint8Array(STROKE_HEADER_BYTES + samples.length * STROKE_POINT_BYTES);
  bytes[0] = STROKE_GEOMETRY_VERSION;
  bytes[1] = tool;
  bytes[2] = directionBits | (hasPressure ? 0b0000_0100 : 0);
  bytes[3] = 0;
  const view = new DataView(bytes.buffer);
  view.setUint32(4, samples.length, true);
  samples.forEach((sample, index) => {
    const offset = STROKE_HEADER_BYTES + index * STROKE_POINT_BYTES;
    view.setUint16(offset, quantiseUnit(sample.u), true);
    view.setUint16(offset + 2, quantiseUnit(sample.v), true);
    bytes[offset + 4] = Math.round(clamp(sample.pressure ?? 0, 0, 1) * 0xff);
  });
  return bytes;
}

/**
 * 单点半径：带压感时按 `0.15 + 0.85 × pressure` 收细。
 *
 * 「无压感回退固定线宽」在**整笔**层面判断（`hasPressure === false`）：写入侧只在
 * 确实收到数位笔压感时才置位，因此整笔要么全按压感画、要么全等宽。笔迹内部的
 * `pressure === 0` 就是「这一下几乎没使劲」，按最细的一档画——早期实现把它当成
 * 「设备没给数据」而回退成满宽，结果是轻描反而最粗。
 */
export function strokePointRadius(
  strokeWidth: number,
  pressure: number,
  hasPressure: boolean,
): number {
  const base = Math.max(strokeWidth, 0.01) / 2;
  if (!hasPressure) return base;
  return base * (0.15 + 0.85 * clamp(pressure, 0, 1));
}

/**
 * 亚像素抽稀（LOD）：相邻点在世界空间的距离不足 `minDistance` 时丢弃后一个点。
 * 首尾点始终保留，因此整笔不会因为缩小而断裂——低缩放下一条 300 点的笔画会收成
 * 十来个点，形状仍在，线段数量却掉了一个量级。
 */
function decimateStrokeSamples<T extends { point: Point }>(
  entries: readonly T[],
  minDistance: number,
): T[] {
  if (entries.length <= 2 || !(minDistance > 0)) return [...entries];
  const kept: T[] = [entries[0]];
  let last = entries[0];
  for (let index = 1; index < entries.length - 1; index += 1) {
    const entry = entries[index];
    if (Math.hypot(entry.point[0] - last.point[0], entry.point[1] - last.point[1]) >= minDistance) {
      kept.push(entry);
      last = entry;
    }
  }
  const tail = entries[entries.length - 1];
  if (kept[kept.length - 1] !== tail) kept.push(tail);
  return kept;
}

const CANONICAL_UV: Quad = [[0, 0], [1, 0], [1, 1], [0, 1]];

/**
 * 与局部 (0,0)/(1,0)/(1,1)/(0,1) 对应的世界角点。
 *
 * 条目的 `uv` 决定「局部坐标的哪个角对应哪个世界角点」：恒等 `uv` 时就是
 * `points` 本身，左右/上下镜像后同一份映射会让笔迹跟着镜像（与图片一致）。
 */
export function strokeWorldCorners(points: Quad, uv: Quad): Quad {
  const used = new Set<number>();
  return CANONICAL_UV.map(([u, v]) => {
    let best = 0;
    let bestDistance = Number.POSITIVE_INFINITY;
    for (let index = 0; index < points.length; index += 1) {
      if (used.has(index)) continue;
      const distance = (uv[index][0] - u) ** 2 + (uv[index][1] - v) ** 2;
      if (distance < bestDistance) {
        bestDistance = distance;
        best = index;
      }
    }
    used.add(best);
    return [...points[best]] as Point;
  }) as Quad;
}

/** 局部坐标 → 世界坐标（四角双线性映射）。 */
export function mapLocalPoint(
  corners: Quad,
  local: Point,
  width: number,
  height: number,
): Point {
  const u = width > 0 ? local[0] / width : 0;
  const v = height > 0 ? local[1] / height : 0;
  const inverseU = 1 - u;
  const inverseV = 1 - v;
  return [
    corners[0][0] * inverseU * inverseV
      + corners[1][0] * u * inverseV
      + corners[3][0] * inverseU * v
      + corners[2][0] * u * v,
    corners[0][1] * inverseU * inverseV
      + corners[1][1] * u * inverseV
      + corners[3][1] * inverseU * v
      + corners[2][1] * u * v,
  ];
}

/**
 * 构建整板笔迹的顶点/索引缓冲。返回的空批次（`indices.length === 0`）表示没有可画的
 * 笔迹，调用方应跳过这一趟绘制。
 */
export function buildStrokeBatch(
  entries: readonly StrokeDrawEntry[],
  options: StrokeBatchOptions,
): StrokeBatch {
  const pixelsPerWorldUnit = Math.max(options.pixelsPerWorldUnit, 1e-6);
  const pad = Math.max(1 / pixelsPerWorldUnit, STROKE_MIN_AA_WORLD_UNITS);
  const vertices: number[] = [];
  const indices: number[] = [];
  let primitiveCount = 0;
  let strokeCount = 0;

  for (const { item, geometry } of entries) {
    if (!(item.width > 0) || !(item.height > 0)) continue;
    const corners = strokeWorldCorners(item.points, item.uv);
    const primitives = itemPrimitives(item, geometry, pixelsPerWorldUnit, pad);
    if (primitives.length === 0) continue;
    const color = hexToRgba(item.color);
    let emitted = 0;
    for (const primitive of primitives) {
      const world = primitive.quad.map((corner) => (
        mapLocalPoint(corners, corner, item.width, item.height)
      )) as Quad;
      // 逐段裁剪：整块四边形都在视口（含超扫边距）之外时直接跳过，不写顶点。
      if (options.cullBounds && !quadIntersectsBounds(world, options.cullBounds)) continue;
      const base = vertices.length / STROKE_VERTEX_FLOATS;
      for (let index = 0; index < world.length; index += 1) {
        const corner = primitive.quad[index];
        vertices.push(
          world[index][0],
          world[index][1],
          corner[0],
          corner[1],
          primitive.a[0],
          primitive.a[1],
          primitive.b[0],
          primitive.b[1],
          primitive.c[0],
          primitive.c[1],
          primitive.radius[0],
          primitive.radius[1],
          color[0],
          color[1],
          color[2],
          color[3],
          primitive.kind,
          0,
        );
      }
      indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
      primitiveCount += 1;
      emitted += 1;
    }
    // 只有真正写出图元的笔迹才计入：被裁剪掉的整笔不进 `strokeCount`，
    // 调用方据此判断「这一趟有没有东西要画」。
    if (emitted > 0) strokeCount += 1;
  }

  return {
    vertices: new Float32Array(vertices),
    indices: new Uint32Array(indices),
    primitiveCount,
    strokeCount,
  };
}

/**
 * 选中描边用的 line-strip 顶点：每笔 5 个点（闭合四边形），按 `index` 顺序连续排布，
 * 绘制时用 `draw(5, 1, index * 5)` 取对应的那一段。
 */
export function buildStrokeOutlines(
  items: readonly Pick<PinBoardStrokeItem, "itemId" | "points">[],
): { vertices: Float32Array; index: Map<number, number> } {
  const vertices = new Float32Array(items.length * STROKE_OUTLINE_VERTICES * 2);
  const index = new Map<number, number>();
  items.forEach((item, slot) => {
    index.set(item.itemId, slot);
    const base = slot * STROKE_OUTLINE_VERTICES * 2;
    for (let step = 0; step < STROKE_OUTLINE_VERTICES; step += 1) {
      const point = item.points[step % 4];
      vertices[base + step * 2] = point[0];
      vertices[base + step * 2 + 1] = point[1];
    }
  });
  return { vertices, index };
}

function itemPrimitives(
  item: PinBoardStrokeItem,
  geometry: PinBoardStrokeGeometry,
  pixelsPerWorldUnit: number,
  pad: number,
): StrokePrimitive[] {
  const halfWidth = Math.max(item.strokeWidth, 0.01) / 2;
  if (geometry.tool === STROKE_TOOL.pen) {
    return penPrimitives(item, geometry, pixelsPerWorldUnit, pad);
  }
  const [start, end] = shapeAnchors(item, geometry.direction, halfWidth);
  switch (geometry.tool) {
    case STROKE_TOOL.line: {
      const segment = segmentPrimitive(start, end, halfWidth, halfWidth, pad);
      return segment ? [segment] : [];
    }
    case STROKE_TOOL.arrow: {
      const head = arrowHeadPoints(start, end, item.strokeWidth);
      if (!head) return [];
      // 杆止于三角形底边：杆末端的圆头藏在箭头里，不会从箭尖后面冒出来。
      const shaft = segmentPrimitive(start, head.base, halfWidth, halfWidth, pad);
      const triangle = trianglePrimitive(end, head.left, head.right, pad);
      return shaft ? [shaft, triangle] : [triangle];
    }
    case STROKE_TOOL.rect:
      return [rectPrimitive(item, halfWidth, pad)];
    case STROKE_TOOL.ellipse:
      return [ellipsePrimitive(item, halfWidth, pad)];
    default:
      return [];
  }
}

function penPrimitives(
  item: PinBoardStrokeItem,
  geometry: PinBoardStrokeGeometry,
  pixelsPerWorldUnit: number,
  pad: number,
): StrokePrimitive[] {
  const samples = decodeStrokeSamples(geometry);
  if (samples.length === 0) return [];
  const entries = samples.map((sample) => ({
    point: [sample.u * item.width, sample.v * item.height] as Point,
    radius: strokePointRadius(item.strokeWidth, sample.pressure, geometry.hasPressure),
  }));
  // 低缩放下把亚像素间隔的点并掉：既省线段，也避免逐段独立绘制在极短段上抖动。
  const kept = decimateStrokeSamples(entries, 1 / pixelsPerWorldUnit);
  const primitives: StrokePrimitive[] = [];
  for (let index = 1; index < kept.length; index += 1) {
    const previous = kept[index - 1];
    const current = kept[index];
    const segment = segmentPrimitive(
      previous.point,
      current.point,
      previous.radius,
      current.radius,
      pad,
    );
    if (segment) primitives.push(segment);
  }
  return primitives;
}

function segmentPrimitive(
  a: Point,
  b: Point,
  radiusA: number,
  radiusB: number,
  pad: number,
): StrokePrimitive | null {
  const deltaX = b[0] - a[0];
  const deltaY = b[1] - a[1];
  const length = Math.hypot(deltaX, deltaY);
  if (!(length >= MIN_POINT_DISTANCE)) return null;
  const unitX = deltaX / length;
  const unitY = deltaY / length;
  const normalX = -unitY;
  const normalY = unitX;
  // 两端各沿切线外扩一个半径，圆头才不会被四边形切平；外扩量随两端半径变化，
  // 与片元里线性插值的宽度一致。
  const extentA = Math.max(radiusA, 0) + pad;
  const extentB = Math.max(radiusB, 0) + pad;
  return {
    kind: STROKE_PRIMITIVE_KIND.segment,
    quad: [
      [a[0] + normalX * extentA - unitX * extentA, a[1] + normalY * extentA - unitY * extentA],
      [b[0] + normalX * extentB + unitX * extentB, b[1] + normalY * extentB + unitY * extentB],
      [b[0] - normalX * extentB + unitX * extentB, b[1] - normalY * extentB + unitY * extentB],
      [a[0] - normalX * extentA - unitX * extentA, a[1] - normalY * extentA - unitY * extentA],
    ],
    a,
    b,
    c: a,
    radius: [Math.max(radiusA, 0), Math.max(radiusB, 0)],
  };
}

/**
 * 直线与箭头的两端点：包围盒按线宽膨胀后，路径要从四角各内缩半个线宽；方向位决定
 * 起点落在哪个角（0 左上、1 右上、2 右下、3 左下），终点是对角。
 */
export function shapeAnchors(
  item: Pick<PinBoardStrokeItem, "width" | "height">,
  direction: number,
  halfWidth: number,
): [Point, Point] {
  const insetX = Math.min(Math.max(halfWidth, 0), item.width / 2);
  const insetY = Math.min(Math.max(halfWidth, 0), item.height / 2);
  const left = insetX;
  const right = item.width - insetX;
  const top = insetY;
  const bottom = item.height - insetY;
  switch (direction & 0b11) {
    case 1:
      return [[right, top], [left, bottom]];
    case 2:
      return [[right, bottom], [left, top]];
    case 3:
      return [[left, bottom], [right, top]];
    default:
      return [[left, top], [right, bottom]];
  }
}

function arrowHeadPoints(
  start: Point,
  end: Point,
  strokeWidth: number,
): { base: Point; left: Point; right: Point } | null {
  const deltaX = end[0] - start[0];
  const deltaY = end[1] - start[1];
  const length = Math.hypot(deltaX, deltaY);
  if (!(length >= MIN_POINT_DISTANCE)) return null;
  const unitX = deltaX / length;
  const unitY = deltaY / length;
  // 短箭头按实际长度收缩，避免「头比杆长」时从头后面冒出去。
  const headLength = Math.min(length, Math.max(strokeWidth * 4, 1e-4));
  const halfBase = headLength * 0.5;
  const base: Point = [end[0] - unitX * headLength, end[1] - unitY * headLength];
  return {
    base,
    left: [base[0] - unitY * halfBase, base[1] + unitX * halfBase],
    right: [base[0] + unitY * halfBase, base[1] - unitX * halfBase],
  };
}

function trianglePrimitive(
  tip: Point,
  left: Point,
  right: Point,
  pad: number,
): StrokePrimitive {
  return {
    kind: STROKE_PRIMITIVE_KIND.triangle,
    quad: boundsQuad([tip, left, right], pad),
    a: tip,
    b: left,
    c: right,
    radius: [0, 0],
  };
}

/** 矩形的包围盒是墨迹外框，路径从四边各内缩半个线宽。 */
function rectPrimitive(
  item: Pick<PinBoardStrokeItem, "width" | "height">,
  halfWidth: number,
  pad: number,
): StrokePrimitive {
  const insetX = Math.min(halfWidth, item.width / 2);
  const insetY = Math.min(halfWidth, item.height / 2);
  const min: Point = [insetX, insetY];
  const max: Point = [item.width - insetX, item.height - insetY];
  return {
    kind: STROKE_PRIMITIVE_KIND.box,
    quad: boundsQuad([min, max], pad),
    a: min,
    b: max,
    c: min,
    radius: [halfWidth, 0],
  };
}

function ellipsePrimitive(
  item: Pick<PinBoardStrokeItem, "width" | "height">,
  halfWidth: number,
  pad: number,
): StrokePrimitive {
  const center: Point = [item.width / 2, item.height / 2];
  const radii: Point = [
    Math.max(item.width / 2 - halfWidth, 1e-4),
    Math.max(item.height / 2 - halfWidth, 1e-4),
  ];
  return {
    kind: STROKE_PRIMITIVE_KIND.ellipse,
    quad: boundsQuad([
      [center[0] - radii[0], center[1] - radii[1]],
      [center[0] + radii[0], center[1] + radii[1]],
    ], pad),
    a: center,
    b: radii,
    c: center,
    radius: [halfWidth, 0],
  };
}

function boundsQuad(points: readonly Point[], pad: number): Quad {
  let minX = Number.POSITIVE_INFINITY;
  let minY = Number.POSITIVE_INFINITY;
  let maxX = Number.NEGATIVE_INFINITY;
  let maxY = Number.NEGATIVE_INFINITY;
  for (const [x, y] of points) {
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    maxX = Math.max(maxX, x);
    maxY = Math.max(maxY, y);
  }
  const extent = Math.max(pad, STROKE_MIN_AA_WORLD_UNITS);
  return [
    [minX - extent, minY - extent],
    [maxX + extent, minY - extent],
    [maxX + extent, maxY + extent],
    [minX - extent, maxY + extent],
  ];
}

function hexToRgba(color: string): [number, number, number, number] {
  const match = /^#([0-9a-f]{6})$/i.exec(color.trim());
  if (!match) return [1, 1, 1, 1];
  const value = Number.parseInt(match[1], 16);
  return [
    ((value >> 16) & 0xff) / 0xff,
    ((value >> 8) & 0xff) / 0xff,
    (value & 0xff) / 0xff,
    1,
  ];
}

function quantiseUnit(value: number): number {
  return Math.round(clamp(value, 0, 1) * 0xffff);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
