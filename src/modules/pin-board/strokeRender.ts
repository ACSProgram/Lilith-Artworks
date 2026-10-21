/**
 * 笔迹渲染的纯几何层：把 `read_pin_board_strokes` 的几何 BLOB 转成一条可以一次性
 * 提交的 SDF 顶点/索引缓冲。
 *
 * 笔迹不建纹理：每个图元（线段、矩形、椭圆、箭头三角）都是一个贴着图元外框的四边形，
 * 片元着色器在**局部坐标**里算距离场并做 1 px 羽化，因此缩放 10%–800% 时边缘始终
 * 清晰，也不会像「按包围盒开 RGBA 纹理」那样为细长笔迹浪费显存。
 *
 * 两个覆盖口径不能省：
 *
 * - 四边形的**外扩量必须含半个线宽**。距离场是以中心线/路径为基准的，墨迹在路径
 *   两侧各占半个线宽；只按羽化宽度外扩会把外侧半个线宽整条裁掉（矩形落地后突然
 *   变细、椭圆被方框切掉左右两端，都是这个原因）。
 * - 自由笔的相邻段必须**首尾相接**而不是叠在一起。每段各自带一个圆头，两段的重叠
 *   区会把 1 px 羽化混合两次：整条笔迹在重叠处比单段更实，细线缩小时就会沿采样
 *   间隔出现一串亮点（看起来像虚线）。这里改成按角平分线算「斜接」偏移，相邻段
 *   共用同一对顶点，边缘因此只被算一次。
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
/**
 * 椭圆四边形的额外外扩倍数。椭圆距离场用的是「隐函数 / 梯度」近似，长短轴差距越大
 * 低估得越多，所以按半个线宽的 1.6 倍留余量：常见的扁椭圆（长短轴差二十倍以内、线宽
 * 明显小于短半径）都盖得住；再极端的细椭圆仍会被切掉一点，那是近似本身的局限，不是
 * 外扩量不够。
 */
const ELLIPSE_AA_MARGIN = 1.6;
/**
 * 斜接长度上限（相对半径）。`r / cos(θ/2)` 在转角超过约 96° 时会长过 1.5r，这时
 * 宁可把外侧尖角切平一点（斜切），也不让斜接顶点拉出一根长刺。采样管道保留 75° 以上
 * 的尖角（`splitAtCorners`），所以真实笔迹里确实会碰到这种转角。
 */
const MITRE_LIMIT = 1.5;
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
  // 低缩放下把亚像素间隔的点并掉：既省线段，也避免极短段上算出的斜接方向乱跳。
  // 抽稀的阈值在高缩放下可能小于 `MIN_POINT_DISTANCE`，所以再滤一遍完全重合的点：
  // 零长度的段既没有法线，也会让斜接退化成一条刺。
  const kept = decimateStrokeSamples(entries, 1 / pixelsPerWorldUnit)
    .filter((entry, index, all) => index === 0 || isDistinct(all[index - 1].point, entry.point));
  // 所有点重合时没有可画的线段（斜接偏移也拿不到方向），整笔跳过。
  if (kept.length < 2) return [];
  // 每个采样点算一对「斜接」顶点，相邻两段共用，因此面片首尾相接、互不重叠。
  const joints = kept.map((_, index) => joinVertices(kept, index, pad));
  const primitives: StrokePrimitive[] = [];
  for (let index = 1; index < kept.length; index += 1) {
    const previous = kept[index - 1];
    const current = kept[index];
    const start = joints[index - 1];
    const end = joints[index];
    primitives.push({
      kind: STROKE_PRIMITIVE_KIND.segment,
      quad: [start.left, end.left, end.right, start.right],
      a: previous.point,
      b: current.point,
      c: previous.point,
      radius: [previous.radius, current.radius],
    });
  }
  return primitives;
}

/**
 * 采样点两侧的顶点：内点按两侧法线的角平分线斜接（斜接长度 `r / cos(θ/2)`，即到两条
 * 偏移线的交点），首尾点则沿切线外扩一个半径，让圆头完整落在四边形里。两种情形都再
 * 向外加一个 `pad`，给 1 px 羽化留出余量。
 */
function joinVertices(
  entries: readonly { point: Point; radius: number }[],
  index: number,
  pad: number,
): { left: Point; right: Point } {
  const point = entries[index].point;
  const radius = Math.max(entries[index].radius, 0);
  const previous = entries[index - 1];
  const next = entries[index + 1];
  const dirIn = previous ? unitVector(previous.point, point) : null;
  const dirOut = next ? unitVector(point, next.point) : null;
  if (!dirIn || !dirOut) {
    // 端点：法线取唯一一侧的段，沿切线方向多扩一个半径，圆头才不会被切平。
    const direction = dirOut ?? dirIn;
    // 两侧都没有方向只剩一个孤立点，没有可画的面积（`penPrimitives` 已过滤掉）。
    if (!direction) return { left: point, right: point };
    const normal = perpendicular(direction);
    const extent = radius + pad;
    // 起点朝切线反方向外扩（dirIn 为空），终点朝切线正方向外扩。
    const along = dirIn ? 1 : -1;
    return {
      left: [
        point[0] + normal[0] * extent + direction[0] * along * extent,
        point[1] + normal[1] * extent + direction[1] * along * extent,
      ],
      right: [
        point[0] - normal[0] * extent + direction[0] * along * extent,
        point[1] - normal[1] * extent + direction[1] * along * extent,
      ],
    };
  }
  const normalIn = perpendicular(dirIn);
  const normalOut = perpendicular(dirOut);
  let miterX = normalIn[0] + normalOut[0];
  let miterY = normalIn[1] + normalOut[1];
  const length = Math.hypot(miterX, miterY);
  if (length < 1e-6) {
    // 180° 折返：角平分线退化，按单侧法线处理（斜接长度由 `MITRE_LIMIT` 兜住）。
    miterX = normalOut[0];
    miterY = normalOut[1];
  } else {
    miterX /= length;
    miterY /= length;
  }
  const cosHalf = Math.max(miterX * normalIn[0] + miterY * normalIn[1], 0);
  const miterLength = Math.min(radius / Math.max(cosHalf, 0.25), radius * MITRE_LIMIT);
  const extent = miterLength + pad;
  return {
    left: [point[0] + miterX * extent, point[1] + miterY * extent],
    right: [point[0] - miterX * extent, point[1] - miterY * extent],
  };
}

function unitVector(from: Point, to: Point): Point | null {
  const deltaX = to[0] - from[0];
  const deltaY = to[1] - from[1];
  const length = Math.hypot(deltaX, deltaY);
  if (!(length >= MIN_POINT_DISTANCE)) return null;
  return [deltaX / length, deltaY / length];
}

function perpendicular([x, y]: Point): Point {
  return [-y, x];
}

function isDistinct(a: Point, b: Point): boolean {
  return Math.hypot(b[0] - a[0], b[1] - a[1]) >= MIN_POINT_DISTANCE;
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

/**
 * 箭头的杆末端与两个翼尖（局部坐标）。命中判定（`strokeHit.ts`）复用同一份头部
 * 几何，避免「看得到的头」和「擦得到的头」各算一遍而慢慢漂移。
 */
export function arrowHeadPoints(
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
    // 外扩量是「半个线宽 + 羽化余量」：只按羽化外扩会把外侧半个线宽裁掉。
    quad: boundsQuad([min, max], halfWidth + pad),
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
    // 椭圆距离场是近似值，长短轴差距大时会略低估距离；外扩多留一点余量，避免边缘被四边形切平。
    quad: boundsQuad([
      [center[0] - radii[0], center[1] - radii[1]],
      [center[0] + radii[0], center[1] + radii[1]],
    ], halfWidth * ELLIPSE_AA_MARGIN + pad),
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
