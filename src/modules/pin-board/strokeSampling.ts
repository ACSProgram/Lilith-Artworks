/**
 * 自由笔的采样管道：抽稀 → 向心 Catmull-Rom → 三次贝塞尔 → 按弧长重采样 →
 * 压感平滑与两端 taper。
 *
 * 这里只有纯函数，不碰 DOM、不碰 GPU、不读视口：渲染器负责把指针事件换成世界坐标
 * 采样（含 `getCoalescedEvents` 与压感），`strokeDrawing.ts` 负责包围盒与编码。
 *
 * 为什么需要这一层：
 *
 * - Chromium 把 `pointermove` 合并到约 60 Hz，快速划线会丢点；`getCoalescedEvents`
 *   能拿到被合并掉的中间点，但它们本身仍带抖动，直接连成折线会显得毛糙；
 * - 60 Hz 的相邻采样间距常常不到一个像素，属于纯冗余，RDP 抽稀把它们砍掉；
 * - 剩下的点用向心 Catmull-Rom 插值成曲线（过原始点、不过冲），再按弧长重采样成
 *   均匀的点序列——渲染层只会连相邻点，所以曲线必须先拍平成点；
 * - 尖角要保留：转角超过阈值的地方拆成两段，各自插值，角点本身不被抹圆。
 *
 * 所有阈值都按「设备像素」给出再换算成世界单位：用户在哪个缩放下画，密度就按那个
 * 缩放定，因此放大后不会看到折线。
 */

import type { Point } from "./geometry";
import { MAX_STROKE_POINTS } from "./strokeGeometry";

/** 一个采样点：世界坐标 + 压感（0–1，无压感设备恒为 0）。 */
export interface PressureSample {
  point: Point;
  pressure: number;
}

export interface StrokeSamplingOptions {
  /** 设备像素 / 世界单位（绘制时的缩放）。 */
  pixelsPerWorldUnit: number;
  /** 线宽（世界单位）：容差与步长取它的一定比例，粗笔不必留那么多点。 */
  strokeWidth: number;
  /** 是否使用压感（数位笔且确有非零压感）。为 false 时压感全部归零。 */
  usePressure: boolean;
  /** 点数上限，默认 `MAX_STROKE_POINTS`。 */
  maxPoints?: number;
}

/** RDP 容差（设备像素）。1 px 能把 60 Hz 的抖动抹掉，又不会吃掉真实转折。 */
export const RDP_TOLERANCE_CSS_PIXELS = 1;
/**
 * 弧长重采样步长（设备像素）。按「绘制时的缩放」换算成世界单位，因此画的时候多细，
 * 存下来的密度就有多细；2 px 在 6 px 宽的笔迹上看不出折线。
 */
export const SAMPLE_STEP_CSS_PIXELS = 2;
/** 转角超过这个值就当作尖角，拆段插值（不抹圆）。75° 能保住直角与锐角，又不会把
 * 手写曲线上的高曲率段拆成碎片。 */
export const CORNER_ANGLE_RADIANS = (75 * Math.PI) / 180;
/** 压感平滑的半径（点数）：3 点滑动平均，够抹掉压感的锯齿，又不会把起笔的重压抹平。 */
export const PRESSURE_SMOOTHING_RADIUS = 2;
/** 两端 taper 的长度（设备像素）与下限：短于它的笔迹不做 taper，免得整笔被收细。 */
export const TAPER_LENGTH_CSS_PIXELS = 4;
const TAPER_MIN_LENGTH_RATIO = 0.25;
/** taper 末端保留的宽度比例：0.5 表示笔尖收到一半宽，再细就不像同一支笔了。 */
const TAPER_TIP_FACTOR = 0.5;
/** 单段曲线最多拍平多少个点，防住极端长度的退化解。 */
const MAX_CURVE_STEPS = 512;
/** 重合点阈值（世界单位）：指针事件常带完全相同的坐标。 */
const MIN_POINT_DISTANCE = 1e-6;
/** 向心 Catmull-Rom 的指数：0.5 是向心参数化，能避免尖点与自交。 */
const CENTRIPETAL_ALPHA = 0.5;

/** 折线总长（世界单位）。 */
export function polylineLength(samples: readonly PressureSample[]): number {
  let total = 0;
  for (let index = 1; index < samples.length; index += 1) {
    total += distance(samples[index - 1].point, samples[index].point);
  }
  return total;
}

/**
 * Ramer–Douglas–Peucker 抽稀：只保留偏离折线超过 `tolerance` 的点，首尾必留。
 *
 * 用显式栈而不是递归：一次拖拽可能留下几千个点，深递归在长笔画上会爆栈。
 */
export function simplifyPolyline(
  samples: readonly PressureSample[],
  tolerance: number,
): PressureSample[] {
  if (samples.length <= 2 || !(tolerance > 0)) return samples.map(copySample);
  const keep = new Uint8Array(samples.length);
  keep[0] = 1;
  keep[samples.length - 1] = 1;
  const stack: Array<[number, number]> = [[0, samples.length - 1]];
  while (stack.length > 0) {
    const [first, last] = stack.pop()!;
    let farthest = -1;
    let farthestDistance = tolerance;
    for (let index = first + 1; index < last; index += 1) {
      const value = perpendicularDistance(
        samples[index].point,
        samples[first].point,
        samples[last].point,
      );
      if (value > farthestDistance) {
        farthestDistance = value;
        farthest = index;
      }
    }
    if (farthest > 0) {
      keep[farthest] = 1;
      stack.push([first, farthest], [farthest, last]);
    }
  }
  return samples.filter((_, index) => keep[index] === 1).map(copySample);
}

/**
 * 在尖角处把折线拆成多段：每段单独插值，角点同时属于相邻两段（拼接时去重）。
 *
 * 向心 Catmull-Rom 在角点附近会把折角抹圆，拆段后角点成为两段的端点，曲线只从它
 * 出发、不从它上面绕过去。
 */
export function splitAtCorners(samples: readonly PressureSample[]): PressureSample[][] {
  if (samples.length < 3) return [samples.map(copySample)];
  const runs: PressureSample[][] = [];
  let start = 0;
  for (let index = 1; index < samples.length - 1; index += 1) {
    if (turnAngle(samples[index - 1].point, samples[index].point, samples[index + 1].point)
      >= CORNER_ANGLE_RADIANS) {
      runs.push(samples.slice(start, index + 1).map(copySample));
      start = index;
    }
  }
  runs.push(samples.slice(start).map(copySample));
  return runs;
}

/**
 * 向心 Catmull-Rom 的切线换算成三次贝塞尔的两个控制点（Barry–Goldman 形式）。
 *
 * 两端点用镜像点补齐，因此首末段的切线沿相邻段方向，不会向外甩出一个钩子。
 */
export function catmullRomControlPoints(
  p0: Point,
  p1: Point,
  p2: Point,
  p3: Point,
): [Point, Point] {
  const t0 = 0;
  const t1 = knotAfter(t0, p0, p1);
  const t2 = knotAfter(t1, p1, p2);
  const t3 = knotAfter(t2, p2, p3);
  const scale = t2 - t1;
  // m = (t2 - t1) × [(p_next - p_prev) / Δt_in - (p_far - p_prev) / Δt_span
  //                 + (p_far - p_next) / Δt_out]
  const m1: Point = [
    scale * (
      (p1[0] - p0[0]) / (t1 - t0)
      - (p2[0] - p0[0]) / (t2 - t0)
      + (p2[0] - p1[0]) / (t2 - t1)
    ),
    scale * (
      (p1[1] - p0[1]) / (t1 - t0)
      - (p2[1] - p0[1]) / (t2 - t0)
      + (p2[1] - p1[1]) / (t2 - t1)
    ),
  ];
  const m2: Point = [
    scale * (
      (p2[0] - p1[0]) / (t2 - t1)
      - (p3[0] - p1[0]) / (t3 - t1)
      + (p3[0] - p2[0]) / (t3 - t2)
    ),
    scale * (
      (p2[1] - p1[1]) / (t2 - t1)
      - (p3[1] - p1[1]) / (t3 - t1)
      + (p3[1] - p2[1]) / (t3 - t2)
    ),
  ];
  return [
    [p1[0] + m1[0] / 3, p1[1] + m1[1] / 3],
    [p2[0] - m2[0] / 3, p2[1] - m2[1] / 3],
  ];
}

/** 三次贝塞尔在 `t` 处的点。 */
export function cubicPoint(
  start: Point,
  control1: Point,
  control2: Point,
  end: Point,
  t: number,
): Point {
  const inverse = 1 - t;
  const a = inverse * inverse * inverse;
  const b = 3 * inverse * inverse * t;
  const c = 3 * inverse * t * t;
  const d = t * t * t;
  return [
    a * start[0] + b * control1[0] + c * control2[0] + d * end[0],
    a * start[1] + b * control1[1] + c * control2[1] + d * end[1],
  ];
}

/**
 * 把一段折线拍平成曲线上的密集点（压感按 `t` 线性插值）。
 *
 * 两个点的段不拍平——直线插值成曲线没有任何收益，只会把点数翻几十倍。
 */
export function flattenRun(run: readonly PressureSample[], step: number): PressureSample[] {
  if (run.length === 0) return [];
  if (run.length === 1) return [copySample(run[0])];
  if (run.length === 2) return run.map(copySample);
  const dense: PressureSample[] = [];
  const usableStep = Math.max(step, MIN_POINT_DISTANCE);
  for (let index = 0; index + 1 < run.length; index += 1) {
    const previous = run[index].point;
    const next = run[index + 1].point;
    const before = index > 0 ? run[index - 1].point : mirror(previous, next);
    const after = index + 2 < run.length ? run[index + 2].point : mirror(next, previous);
    const [control1, control2] = catmullRomControlPoints(before, previous, next, after);
    const approximate = distance(previous, control1)
      + distance(control1, control2)
      + distance(control2, next);
    const steps = Math.min(MAX_CURVE_STEPS, Math.max(2, Math.ceil(approximate / usableStep)));
    for (let stepIndex = 0; stepIndex < steps; stepIndex += 1) {
      const t = stepIndex / steps;
      dense.push({
        point: cubicPoint(previous, control1, control2, next, t),
        pressure: lerp(run[index].pressure, run[index + 1].pressure, t),
      });
    }
  }
  dense.push(copySample(run[run.length - 1]));
  return dense;
}

/**
 * 按弧长等距重采样：沿折线每 `step` 取一个点，首尾点原样保留。
 *
 * 贝塞尔的参数 `t` 与弧长不成比例（曲率大的地方点更密），重采样之后点距才均匀，
 * 压感平滑与渲染的段长也才有意义。
 */
export function resampleByArcLength(
  samples: readonly PressureSample[],
  step: number,
): PressureSample[] {
  if (samples.length <= 2) return samples.map(copySample);
  const usableStep = Math.max(step, MIN_POINT_DISTANCE);
  const result: PressureSample[] = [copySample(samples[0])];
  // `travelled` 是从起点算起的累计弧长；下一个采样点落在它的整数倍上。
  let travelled = 0;
  let target = usableStep;
  for (let index = 1; index < samples.length; index += 1) {
    const from = samples[index - 1];
    const to = samples[index];
    const segment = distance(from.point, to.point);
    if (!(segment > 0)) continue;
    while (target <= travelled + segment) {
      const ratio = (target - travelled) / segment;
      result.push({
        point: [
          lerp(from.point[0], to.point[0], ratio),
          lerp(from.point[1], to.point[1], ratio),
        ],
        pressure: lerp(from.pressure, to.pressure, ratio),
      });
      target += usableStep;
    }
    travelled += segment;
  }
  const tail = samples[samples.length - 1];
  const last = result[result.length - 1];
  if (distance(last.point, tail.point) > MIN_POINT_DISTANCE) result.push(copySample(tail));
  else result[result.length - 1] = copySample(tail);
  return result;
}

/** 压感的滑动平均（窗口 `2 × radius + 1`，边缘按可用点数平均）。 */
export function smoothPressures(
  samples: readonly PressureSample[],
  radius = PRESSURE_SMOOTHING_RADIUS,
): PressureSample[] {
  if (samples.length < 3 || radius < 1) return samples.map(copySample);
  return samples.map((sample, index) => {
    let total = 0;
    let count = 0;
    for (let offset = -radius; offset <= radius; offset += 1) {
      const neighbour = samples[index + offset];
      if (!neighbour) continue;
      total += neighbour.pressure;
      count += 1;
    }
    return { point: [...sample.point] as Point, pressure: total / count };
  });
}

/**
 * 两端 taper：笔尖与收笔处把压感按余弦斜坡压到 `TAPER_TIP_FACTOR` 倍。
 *
 * 太短的笔迹（不足 `taperLength / TAPER_MIN_LENGTH_RATIO`）不做 taper——否则整笔都
 * 落在斜坡里，会被均匀收细。
 */
export function applyTaper(
  samples: readonly PressureSample[],
  taperLength: number,
): PressureSample[] {
  if (samples.length < 4 || !(taperLength > 0)) return samples.map(copySample);
  const total = polylineLength(samples);
  if (total < taperLength / TAPER_MIN_LENGTH_RATIO) return samples.map(copySample);
  const distances = new Array<number>(samples.length);
  distances[0] = 0;
  for (let index = 1; index < samples.length; index += 1) {
    distances[index] = distances[index - 1] + distance(samples[index - 1].point, samples[index].point);
  }
  return samples.map((sample, index) => {
    const fromStart = Math.min(distances[index] / taperLength, 1);
    const fromEnd = Math.min((total - distances[index]) / taperLength, 1);
    const factor = TAPER_TIP_FACTOR
      + (1 - TAPER_TIP_FACTOR) * smoothstep(Math.min(fromStart, fromEnd));
    return { point: [...sample.point] as Point, pressure: sample.pressure * factor };
  });
}

/**
 * 完整的自由笔采样管道。返回的点已经拍平、均匀、压感平滑并做过 taper，调用方直接
 * 按包围盒归一化后编码即可。
 *
 * 点数上限在这里兜住：先按长度放宽步长，最后再按步长抽稀一次，保证写出的几何一定
 * 落在 `MAX_STROKE_POINTS` 之内（写入侧上限见 `strokeGeometry.ts`）。
 */
export function normalisePenSamples(
  samples: readonly PressureSample[],
  options: StrokeSamplingOptions,
): PressureSample[] {
  const limit = Math.max(2, Math.floor(options.maxPoints ?? MAX_STROKE_POINTS));
  const cleaned = dedupe(samples);
  if (cleaned.length < 2) return cleaned;
  const pixelsPerWorldUnit = Math.max(options.pixelsPerWorldUnit, 1e-6);
  const worldPerPixel = 1 / pixelsPerWorldUnit;
  const strokeWidth = Math.max(options.strokeWidth, 0.01);
  const tolerance = Math.max(
    worldPerPixel * RDP_TOLERANCE_CSS_PIXELS,
    strokeWidth * 0.02,
  );
  const simplified = simplifyPolyline(cleaned, tolerance);
  // 步长同时受点数上限约束：几何越长，点距越粗，宁可略糙也不写出超限的 BLOB。
  const targetLength = polylineLength(simplified);
  const step = Math.max(
    worldPerPixel * SAMPLE_STEP_CSS_PIXELS,
    strokeWidth * 0.1,
    targetLength / (limit - 2),
  );

  const flattened: PressureSample[] = [];
  for (const run of splitAtCorners(simplified)) {
    const resampled = resampleByArcLength(flattenRun(run, step), step);
    if (resampled.length === 0) continue;
    // 相邻两段共享角点，拼接时去掉重复的那个。
    if (flattened.length > 0) resampled.shift();
    flattened.push(...resampled);
  }
  const points = flattened.length >= 2 ? flattened : cleaned;

  let shaped: PressureSample[];
  if (options.usePressure) {
    const taperLength = Math.max(
      worldPerPixel * TAPER_LENGTH_CSS_PIXELS,
      strokeWidth * 0.5,
    );
    shaped = applyTaper(smoothPressures(points), taperLength);
  } else {
    shaped = points.map((sample) => ({ point: sample.point, pressure: 0 }));
  }
  return limitSamples(shaped, limit);
}

/** 均匀抽稀到 `limit` 个点（首尾保留），只作为点数上限的兜底。 */
export function limitSamples(
  samples: readonly PressureSample[],
  limit: number,
): PressureSample[] {
  if (samples.length <= limit) return samples.map(copySample);
  const kept: PressureSample[] = [];
  const stride = (samples.length - 1) / (limit - 1);
  for (let index = 0; index < limit - 1; index += 1) {
    kept.push(copySample(samples[Math.round(index * stride)]));
  }
  kept.push(copySample(samples[samples.length - 1]));
  return kept;
}

function dedupe(samples: readonly PressureSample[]): PressureSample[] {
  const result: PressureSample[] = [];
  for (const sample of samples) {
    const last = result[result.length - 1];
    if (last && distance(last.point, sample.point) < MIN_POINT_DISTANCE) continue;
    result.push(copySample(sample));
  }
  return result;
}

/** 点到线段的垂距；两端重合时退化成点距。 */
function perpendicularDistance(point: Point, start: Point, end: Point): number {
  const deltaX = end[0] - start[0];
  const deltaY = end[1] - start[1];
  const lengthSquared = deltaX * deltaX + deltaY * deltaY;
  if (!(lengthSquared > 0)) return distance(point, start);
  const projection = ((point[0] - start[0]) * deltaX + (point[1] - start[1]) * deltaY) / lengthSquared;
  const clamped = Math.min(1, Math.max(0, projection));
  return Math.hypot(
    point[0] - (start[0] + deltaX * clamped),
    point[1] - (start[1] + deltaY * clamped),
  );
}

/** 折线在 `middle` 处的转角（0 = 直行，π = 原路折返）。 */
function turnAngle(previous: Point, middle: Point, next: Point): number {
  const inX = middle[0] - previous[0];
  const inY = middle[1] - previous[1];
  const outX = next[0] - middle[0];
  const outY = next[1] - middle[1];
  const inLength = Math.hypot(inX, inY);
  const outLength = Math.hypot(outX, outY);
  if (!(inLength > 0) || !(outLength > 0)) return 0;
  const cosine = Math.min(1, Math.max(-1, (inX * outX + inY * outY) / (inLength * outLength)));
  return Math.acos(cosine);
}

function knotAfter(previous: number, from: Point, to: Point): number {
  return previous + Math.max(distance(from, to) ** CENTRIPETAL_ALPHA, 1e-6);
}

function mirror(anchor: Point, other: Point): Point {
  return [2 * anchor[0] - other[0], 2 * anchor[1] - other[1]];
}

function copySample(sample: PressureSample): PressureSample {
  return { point: [sample.point[0], sample.point[1]], pressure: sample.pressure };
}

function distance(left: Point, right: Point): number {
  return Math.hypot(right[0] - left[0], right[1] - left[1]);
}

function lerp(from: number, to: number, ratio: number): number {
  return from + (to - from) * ratio;
}

function smoothstep(value: number): number {
  const clamped = Math.min(1, Math.max(0, value));
  return clamped * clamped * (3 - 2 * clamped);
}
