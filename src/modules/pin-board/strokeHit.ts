/**
 * 笔迹的墨迹命中：把条目几何展开成**世界空间的折线**，再按「墨迹半径 + 容差」判断
 * 命中圆是否碰到墨迹、命中矩形是否与墨迹相交。橡皮擦、点选与框选共用这一套口径。
 *
 * 为什么在世界空间而不是条目局部空间里判：
 *
 * - 条目可以被旋转、镜像（`uv` 翻转）、非等比缩放，局部空间里算出的圆在世界空间是
 *   椭圆，逆变换回来再算距离只会引入近似；
 * - 自由笔的归一化点、形状的两端点本来就要经 `mapLocalPoint` 落到世界（渲染与擦除
 *   共用同一份映射），折线路径因此不需要第二套坐标约定。
 *
 * 两种「覆盖口径」与渲染一致：自由笔每点带自己的压感半径，形状统一用半个线宽；
 * 箭头除了杆还有三角头，从侧面擦箭头必须擦得掉，否则用户只会觉得没点中。
 *
 * 这里只放纯计算。缓存（按条目 id + 几何版本 + 变换签名）由渲染器持有，本模块
 * 不保存任何状态。
 */

import { imageBounds, type Bounds, type Point } from "./geometry";
import { STROKE_TOOL, type PinBoardStrokeGeometry } from "./strokeGeometry";
import {
  arrowHeadPoints,
  decodeStrokeSamples,
  mapLocalPoint,
  shapeAnchors,
  strokePointRadius,
  strokeWorldCorners,
} from "./strokeRender";
import type { PinBoardStrokeItem } from "./types";

/** 椭圆按折线近似的段数：擦除只判「有没有碰到」，不需要像素级贴合。 */
const ELLIPSE_SEGMENTS = 64;

/**
 * 点选与框选的最小命中容差（CSS 像素）。
 *
 * 墨迹判定按真实线宽，但 1–3 px 的细线只按半个线宽判，光标偏一个像素就点不中——
 * 触控板与高 DPI 下尤其明显。因此**选择**的命中在墨迹半径之外再加一条固定容差；
 * 橡皮擦不吃这条容差（擦头半径本身就是用户选定的大小，加大等于暗自改粗细）。
 */
export const STROKE_HIT_TOLERANCE_CSS_PIXELS = 3;

/**
 * 一条世界折线及其每点的墨迹半径（世界单位），`radii` 与 `points` 等长。
 * `closed` 为真时最后一点会连回第一点（矩形与椭圆）。
 */
export interface StrokeWorldOutline {
  points: Point[];
  radii: number[];
  closed: boolean;
}

/**
 * 条目的墨迹外框之外再扩一个命中半径（擦头半径或选择容差）：命中区落在这个框外时
 * 一定碰不到，不必解析几何。预筛是热路径上最省的一次判断。
 */
export function strokeHitBounds(item: PinBoardStrokeItem, radius: number): Bounds {
  const bounds = imageBounds(item);
  const pad = Math.max(radius, 0);
  return {
    minX: bounds.minX - pad,
    minY: bounds.minY - pad,
    maxX: bounds.maxX + pad,
    maxY: bounds.maxY + pad,
  };
}

/**
 * 条目几何 → 世界折线。
 *
 * - 自由笔：归一化点经四角双线性映射落到世界，半径按压感取（整笔无压感时恒为半线宽）；
 * - 直线：两端点一条折线；
 * - 箭头：杆一条折线 + 头部三角（闭合），头比杆宽，只算杆会擦不掉翼尖；
 * - 矩形：按半个线宽内缩后的四边，闭合；
 * - 椭圆：内缩半个线宽后的椭圆按固定段数采样，闭合。
 */
export function buildStrokeWorldOutlines(
  item: PinBoardStrokeItem,
  geometry: PinBoardStrokeGeometry,
): StrokeWorldOutline[] {
  if (!(item.width > 0) || !(item.height > 0)) return [];
  const corners = strokeWorldCorners(item.points, item.uv);
  const toWorld = (local: Point) => mapLocalPoint(corners, local, item.width, item.height);
  const halfWidth = Math.max(item.strokeWidth, 0.01) / 2;

  if (geometry.tool === STROKE_TOOL.pen) {
    const samples = decodeStrokeSamples(geometry);
    if (samples.length === 0) return [];
    const points = samples.map((sample) => toWorld([sample.u * item.width, sample.v * item.height]));
    const radii = samples.map((sample) => (
      strokePointRadius(item.strokeWidth, sample.pressure, geometry.hasPressure)
    ));
    return [{ points, radii, closed: false }];
  }

  const [start, end] = shapeAnchors(item, geometry.direction, halfWidth);
  const radii = (count: number) => new Array<number>(count).fill(halfWidth);
  switch (geometry.tool) {
    case STROKE_TOOL.line:
      return [{
        points: [toWorld(start), toWorld(end)],
        radii: radii(2),
        closed: false,
      }];
    case STROKE_TOOL.arrow: {
      const head = arrowHeadPoints(start, end, item.strokeWidth);
      const outlines: StrokeWorldOutline[] = [{
        // 杆止于三角底边，与渲染一致。
        points: [toWorld(start), toWorld(head?.base ?? end)],
        radii: radii(2),
        closed: false,
      }];
      if (head) {
        outlines.push({
          points: [toWorld(end), toWorld(head.left), toWorld(head.right)],
          radii: radii(3),
          closed: true,
        });
      }
      return outlines;
    }
    case STROKE_TOOL.rect: {
      const insetX = Math.min(halfWidth, item.width / 2);
      const insetY = Math.min(halfWidth, item.height / 2);
      return [{
        points: [
          toWorld([insetX, insetY]),
          toWorld([item.width - insetX, insetY]),
          toWorld([item.width - insetX, item.height - insetY]),
          toWorld([insetX, item.height - insetY]),
        ],
        radii: radii(4),
        closed: true,
      }];
    }
    case STROKE_TOOL.ellipse: {
      const center: Point = [item.width / 2, item.height / 2];
      const radiusX = Math.max(item.width / 2 - halfWidth, 1e-4);
      const radiusY = Math.max(item.height / 2 - halfWidth, 1e-4);
      const points: Point[] = [];
      for (let index = 0; index < ELLIPSE_SEGMENTS; index += 1) {
        const angle = (index / ELLIPSE_SEGMENTS) * Math.PI * 2;
        points.push(toWorld([
          center[0] + Math.cos(angle) * radiusX,
          center[1] + Math.sin(angle) * radiusY,
        ]));
      }
      return [{ points, radii: radii(points.length), closed: true }];
    }
    default:
      return [];
  }
}

/**
 * 圆形命中区是否碰到任一折线的墨迹。
 *
 * `radius` 可以是擦头半径（橡皮擦），也可以是点选的固定容差，两者都只是「在墨迹
 * 半径之外再放宽多少」。线段半径按两端取较大值，宁可多命中一条也不错判。
 */
export function strokeHitByCircle(
  outlines: readonly StrokeWorldOutline[],
  center: Point,
  radius: number,
): boolean {
  const head = Math.max(radius, 0);
  for (const outline of outlines) {
    const { points, radii } = outline;
    if (points.length === 1) {
      const tolerance = head + Math.max(radii[0] ?? 0, 0);
      if (Math.hypot(center[0] - points[0][0], center[1] - points[0][1]) <= tolerance) {
        return true;
      }
      continue;
    }
    const last = outline.closed ? points.length : points.length - 1;
    for (let index = 0; index < last; index += 1) {
      const next = (index + 1) % points.length;
      const tolerance = head + Math.max(radii[index] ?? 0, radii[next] ?? 0);
      if (distanceToSegment(center, points[index], points[next]) <= tolerance) return true;
    }
  }
  return false;
}

/**
 * 墨迹是否被「扫过的擦头」碰到：把指针从上一帧到这一帧的位移当成一段，判墨迹折线到
 * 这一段的距离是否 ≤ 「墨迹半径 + 擦头半径」。
 *
 * 只测当前点会漏擦：浏览器把 `pointermove` 合并到约 60–120 Hz，快速擦动时相邻两帧可能
 * 隔几十个像素，细笔迹落在两次采样之间时整条都擦不到——表现就是「反复擦没反应、点一下
 * 反而擦掉」。`path` 是这一帧的指针轨迹（至少一个点）；单点时退化为圆形判定。
 */
export function strokeHitBySweep(
  outlines: readonly StrokeWorldOutline[],
  path: readonly Point[],
  radius: number,
): boolean {
  if (path.length === 0) return false;
  if (path.length === 1) return strokeHitByCircle(outlines, path[0], radius);
  const head = Math.max(radius, 0);
  for (const outline of outlines) {
    const { points, radii } = outline;
    if (points.length === 0) continue;
    const single = points.length === 1;
    const last = single ? 1 : (outline.closed ? points.length : points.length - 1);
    for (let index = 0; index < last; index += 1) {
      const next = single ? 0 : (index + 1) % points.length;
      const tolerance = head + Math.max(radii[index] ?? 0, radii[next] ?? 0);
      for (let step = 0; step < path.length - 1; step += 1) {
        if (segmentDistance(points[index], points[next], path[step], path[step + 1]) <= tolerance) {
          return true;
        }
      }
    }
  }
  return false;
}

/**
 * 墨迹折线是否与轴对齐矩形相交（框选用），`tolerance` 与点选同口径。
 *
 * 每段按「两端半径的较大值 + 容差」外扩矩形再求交：把墨迹当成有宽度的胶囊，而不是
 * 一条零宽的线。因此框住矩形笔迹的**内部**不算命中——这正是「实际碰撞箱」的含义。
 */
export function strokeIntersectsBounds(
  outlines: readonly StrokeWorldOutline[],
  bounds: Bounds,
  tolerance = 0,
): boolean {
  const pad = Math.max(tolerance, 0);
  for (const outline of outlines) {
    const { points, radii } = outline;
    if (points.length === 0) continue;
    if (points.length === 1) {
      const radius = Math.max(radii[0] ?? 0, 0) + pad;
      if (pointNearBounds(points[0], bounds, radius)) return true;
      continue;
    }
    const last = outline.closed ? points.length : points.length - 1;
    for (let index = 0; index < last; index += 1) {
      const next = (index + 1) % points.length;
      const radius = Math.max(radii[index] ?? 0, radii[next] ?? 0, 0) + pad;
      if (segmentIntersectsBounds(points[index], points[next], bounds, radius)) return true;
    }
  }
  return false;
}

/**
 * 折线缓存的失效签名：几何版本 + 条目的四角与 uv。
 *
 * 几何版本由渲染器在重读笔迹时推进；变换（移动/旋转/缩放/翻转）只改 `points` 与
 * `uv`，因此两者一起构成完整的缓存键。
 */
export function strokeOutlineSignature(
  item: Pick<PinBoardStrokeItem, "points" | "uv">,
  geometryVersion: number,
): string {
  return `${geometryVersion}|${item.points.flat().join(",")}|${item.uv.flat().join(",")}`;
}

function distanceToSegment(point: Point, start: Point, end: Point): number {
  const deltaX = end[0] - start[0];
  const deltaY = end[1] - start[1];
  const lengthSquared = deltaX * deltaX + deltaY * deltaY;
  if (!(lengthSquared > 0)) {
    return Math.hypot(point[0] - start[0], point[1] - start[1]);
  }
  let ratio = ((point[0] - start[0]) * deltaX + (point[1] - start[1]) * deltaY) / lengthSquared;
  ratio = Math.min(1, Math.max(0, ratio));
  return Math.hypot(
    point[0] - (start[0] + deltaX * ratio),
    point[1] - (start[1] + deltaY * ratio),
  );
}

function pointNearBounds(point: Point, bounds: Bounds, pad: number): boolean {
  return point[0] >= bounds.minX - pad
    && point[0] <= bounds.maxX + pad
    && point[1] >= bounds.minY - pad
    && point[1] <= bounds.maxY + pad;
}

/**
 * 线段与「外扩了 `pad` 的矩形」是否相交。
 *
 * 先短路再求交：两端点有任一个落在框内 → 命中；线段包围盒与矩形不相交 → 直接否；
 * 剩下的只有「线段穿过矩形某条边」这一种可能，做四次边求交即可。框选是每个指针事件
 * 都要跑的路径，这几次短路决定了拖框时的手感。
 */
function segmentIntersectsBounds(start: Point, end: Point, bounds: Bounds, pad: number): boolean {
  const minX = bounds.minX - pad;
  const maxX = bounds.maxX + pad;
  const minY = bounds.minY - pad;
  const maxY = bounds.maxY + pad;
  if (pointNearBounds(start, bounds, pad) || pointNearBounds(end, bounds, pad)) return true;
  if (Math.max(start[0], end[0]) < minX || Math.min(start[0], end[0]) > maxX) return false;
  if (Math.max(start[1], end[1]) < minY || Math.min(start[1], end[1]) > maxY) return false;
  const topLeft: Point = [minX, minY];
  const topRight: Point = [maxX, minY];
  const bottomRight: Point = [maxX, maxY];
  const bottomLeft: Point = [minX, maxY];
  return segmentsCross(start, end, topLeft, topRight)
    || segmentsCross(start, end, topRight, bottomRight)
    || segmentsCross(start, end, bottomRight, bottomLeft)
    || segmentsCross(start, end, bottomLeft, topLeft);
}

/** 两条线段的最短距离（相交为 0）；两端都退化成点时等价于点距。 */
function segmentDistance(a1: Point, a2: Point, b1: Point, b2: Point): number {
  if (segmentsCross(a1, a2, b1, b2)) return 0;
  return Math.min(
    distanceToSegment(a1, b1, b2),
    distanceToSegment(a2, b1, b2),
    distanceToSegment(b1, a1, a2),
    distanceToSegment(b2, a1, a2),
  );
}

/**
 * 两条线段是否相交（含端点接触）。
 *
 * 平行（含共线）时返回 false：共线重叠的「相交」在这里不重要——框选的端点判内会先接住，
 * 扫掠的退路是用四条点线距离取最小值，两者都不依赖共线判定。
 */
function segmentsCross(a: Point, b: Point, c: Point, d: Point): boolean {
  const abX = b[0] - a[0];
  const abY = b[1] - a[1];
  const cdX = d[0] - c[0];
  const cdY = d[1] - c[1];
  const denominator = abX * cdY - abY * cdX;
  if (Math.abs(denominator) < 1e-12) return false;
  const acX = c[0] - a[0];
  const acY = c[1] - a[1];
  const along = (acX * cdY - acY * cdX) / denominator;
  const across = (acX * abY - acY * abX) / denominator;
  return along >= 0 && along <= 1 && across >= 0 && across <= 1;
}
