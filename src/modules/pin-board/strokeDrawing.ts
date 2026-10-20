/**
 * 绘制交互的纯状态与几何：工具枚举、Shift 约束、草稿到几何 BLOB 的编码。
 *
 * 这里只放不碰 DOM、不碰 GPU 的部分，渲染器负责指针事件与预览层，控制器负责落库。
 * 两侧共用的几何契约在 `strokeGeometry.ts`，写入侧的包围盒口径必须与
 * `strokeRender.ts` 的读侧一致：
 *
 * - 包围盒是**墨迹外框**，因此自由笔取「点中心极值 ± 半径」，形状取「两端点外扩半个
 *   线宽」——读侧会把路径按半个线宽内缩回墨迹中心；
 * - 形状不存点数据，两端点由包围盒与方向位（0 左上 / 1 右上 / 2 右下 / 3 左下）推出，
 *   方向位按局部坐标计（局部 y 向下，与图片 UV 同一取向）；
 * - 自由笔的点按包围盒归一化，`u` 沿 x、`v` 沿局部 y（顶边为 0）。
 */

import type { Point } from "./geometry";
import {
  MAX_STROKE_EXTENT,
  MAX_STROKE_POINTS,
  STROKE_TOOL,
  type PinBoardStrokeTool,
} from "./strokeGeometry";
import { encodeStrokeGeometry } from "./strokeRender";

/** 可绘制工具；`select` 不在其中，它沿用原有的选中/框选/变换交互。 */
export type StrokeToolName = "pen" | "line" | "arrow" | "rect" | "ellipse";
export type PinBoardTool = "select" | StrokeToolName;

/** 标注的默认颜色取偏红的高对比色：在多数参考图上都能看清，又不至于像纯红那样刺眼。 */
export const DEFAULT_STROKE_COLOR = "#e5484d";
export const DEFAULT_STROKE_WIDTH = 6;
/** 线宽是世界单位（与图片像素同量级），输入框与原生端上限取更保守的一档。 */
export const MIN_STROKE_WIDTH = 1;
export const MAX_STROKE_WIDTH = 64;
/**
 * 最小拖拽距离（CSS 像素）。低于它按「误点」处理：单击不留下一个看不见的条目，
 * 形状也拿不到退化的包围盒。
 */
export const MIN_STROKE_DRAG_CSS_PIXELS = 3;

/** 同一个点重复到达时的合并阈值（世界单位）：指针事件常带完全相同的坐标。 */
const MIN_SAMPLE_DISTANCE = 1e-6;

const TOOL_BY_KEY: Readonly<Record<string, PinBoardTool>> = {
  v: "select",
  p: "pen",
  l: "line",
  a: "arrow",
  r: "rect",
  o: "ellipse",
};

export function isStrokeTool(tool: PinBoardTool): tool is StrokeToolName {
  return tool !== "select";
}

/** 前端工具名 → 几何 BLOB 里的工具编号。 */
export function strokeToolId(tool: StrokeToolName): PinBoardStrokeTool {
  return STROKE_TOOL[tool];
}

/** 单键工具快捷键（`V` 选择、`P` 笔、`L` 直线、`A` 箭头、`R` 矩形、`O` 椭圆）。 */
export function toolForShortcut(key: string): PinBoardTool | null {
  return TOOL_BY_KEY[key.toLowerCase()] ?? null;
}

/** 线宽收敛到输入框与原生端都接受的范围；非有限值回落到默认线宽。 */
export function clampStrokeWidth(width: number): number {
  if (!Number.isFinite(width)) return DEFAULT_STROKE_WIDTH;
  return Math.min(MAX_STROKE_WIDTH, Math.max(MIN_STROKE_WIDTH, width));
}

/** 一次绘制的草稿：世界坐标，与视口无关（缩放/平移不影响已采样的点）。 */
export interface StrokeDraft {
  readonly tool: StrokeToolName;
  /** 自由笔的全部采样点；形状只保留起点。 */
  readonly points: Point[];
  /** 当前末点（未经 Shift 约束）；形状与 Shift 下的自由笔都从这里取末端。 */
  end: Point;
  /** 最后一次指针事件的 Shift 状态。 */
  shift: boolean;
}

export function beginStrokeDraft(tool: StrokeToolName, point: Point): StrokeDraft {
  return {
    tool,
    points: [[point[0], point[1]]],
    end: [point[0], point[1]],
    shift: false,
  };
}

/**
 * 追加一个指针位置。
 *
 * 自由笔在按住 Shift 时不再累计点：那段时间的轨迹就是一条从起点出发的直线，末端由
 * `end` 给出；松开 Shift 后继续从起点往后累计（与常见绘图软件一致）。
 */
export function extendStrokeDraft(draft: StrokeDraft, point: Point, shift: boolean): void {
  draft.end = [point[0], point[1]];
  draft.shift = shift;
  if (draft.tool !== "pen" || shift) return;
  const last = draft.points[draft.points.length - 1];
  if (Math.hypot(point[0] - last[0], point[1] - last[1]) < MIN_SAMPLE_DISTANCE) return;
  draft.points.push([point[0], point[1]]);
}

/** 草稿的有效点序列：形状为 [起点, 约束后的末点]，自由笔为全部采样点。 */
export function strokeDraftPoints(draft: StrokeDraft): readonly Point[] {
  const start = draft.points[0];
  if (draft.tool === "pen") {
    // 直接返回采样数组：预览每个指针事件都要读它，复制一份在长笔画上是纯开销。
    return draft.shift ? [[...start] as Point, [...draft.end] as Point] : draft.points;
  }
  return [[...start] as Point, constrainShapeEnd(draft.tool, start, draft.end, draft.shift)];
}

/**
 * Shift 约束：
 *
 * - 矩形 / 椭圆按正方形 / 圆处理，边长取两轴拖拽量的较大值，方向沿用拖拽方向；
 * - 直线 / 箭头吸附到 45° 的整数倍，拖拽量投影到吸附后的方向上（终点仍在光标那条射线上）；
 * - 自由笔的直线约束在 `extendStrokeDraft` 里处理，这里不参与。
 */
export function constrainShapeEnd(
  tool: StrokeToolName,
  start: Point,
  end: Point,
  shift: boolean,
): Point {
  if (!shift) return [end[0], end[1]];
  const deltaX = end[0] - start[0];
  const deltaY = end[1] - start[1];
  if (tool === "rect" || tool === "ellipse") {
    const side = Math.max(Math.abs(deltaX), Math.abs(deltaY));
    return [
      start[0] + (deltaX < 0 ? -side : side),
      start[1] + (deltaY < 0 ? -side : side),
    ];
  }
  const length = Math.hypot(deltaX, deltaY);
  if (!(length > 0)) return [end[0], end[1]];
  const step = Math.PI / 4;
  const angle = Math.round(Math.atan2(deltaY, deltaX) / step) * step;
  // 把拖拽量投影到吸附后的方向上：终点仍落在光标所在的那条射线上，不会因为
  // 「保留长度」而越过光标。
  const projected = deltaX * Math.cos(angle) + deltaY * Math.sin(angle);
  return [start[0] + Math.cos(angle) * projected, start[1] + Math.sin(angle) * projected];
}

/** 落库所需的几何：包围盒（世界单位）、几何 BLOB 与条目中心点。 */
export interface StrokeCommit {
  tool: StrokeToolName;
  width: number;
  height: number;
  strokeWidth: number;
  geometry: Uint8Array;
  centerX: number;
  centerY: number;
}

/** 提交给控制器的完整请求：几何之外还带颜色。 */
export interface StrokeCommitRequest extends StrokeCommit {
  color: string;
}

/**
 * 把草稿编码成落库请求；拖拽距离不足、包围盒越界或几何无法编码时返回 null
 * （调用方按「取消」处理，不产生条目）。
 *
 * `minimumExtent` 是拖拽距离下限（世界单位），由渲染器按当前缩放换算成像素。
 */
export function resolveStrokeDraft(
  draft: StrokeDraft,
  strokeWidth: number,
  minimumExtent = 0,
): StrokeCommit | null {
  const width = clampStrokeWidth(strokeWidth);
  const points = strokeDraftPoints(draft);
  if (draft.tool === "pen") return penCommit(points, width, minimumExtent);
  return shapeCommit(draft.tool, points[0], points[1], width, minimumExtent);
}

/**
 * 点数超限时按固定步长抽稀（首尾点保留）。阶段 5 的 RDP 抽稀在采样层完成，
 * 这里只是兜住「一次拖拽点数超过上限」的写入侧，避免整笔被拒绝。
 */
export function thinStrokePoints(points: readonly Point[], limit = MAX_STROKE_POINTS): Point[] {
  if (points.length <= limit) return points.map((point) => [...point] as Point);
  const kept: Point[] = [];
  const step = (points.length - 1) / (limit - 1);
  for (let index = 0; index < limit - 1; index += 1) {
    kept.push([...points[Math.round(index * step)]] as Point);
  }
  kept.push([...points[points.length - 1]] as Point);
  return kept;
}

function penCommit(
  samples: readonly Point[],
  strokeWidth: number,
  minimumExtent: number,
): StrokeCommit | null {
  // 单点画不出线段：与其留一个看不见的条目，不如按取消处理。
  if (samples.length < 2) return null;
  const points = thinStrokePoints(samples);
  const bounds = pointsBounds(points);
  if (Math.hypot(bounds.maxX - bounds.minX, bounds.maxY - bounds.minY) < minimumExtent) return null;
  const half = strokeWidth / 2;
  // 包围盒是墨迹外框：点中心极值再向外扩一个半径。
  const left = bounds.minX - half;
  const top = bounds.maxY + half;
  const width = bounds.maxX - bounds.minX + strokeWidth;
  const height = bounds.maxY - bounds.minY + strokeWidth;
  if (!(width > 0) || !(height > 0) || width > MAX_STROKE_EXTENT || height > MAX_STROKE_EXTENT) {
    return null;
  }
  const normalized = points.map(([x, y]) => ({
    u: (x - left) / width,
    // 局部 y 向下：顶边对应世界坐标的最大 y。
    v: (top - y) / height,
    pressure: 0,
  }));
  return {
    tool: "pen",
    width,
    height,
    strokeWidth,
    geometry: encodeStrokeGeometry(STROKE_TOOL.pen, normalized),
    centerX: left + width / 2,
    centerY: top - height / 2,
  };
}

function shapeCommit(
  tool: StrokeToolName,
  start: Point,
  end: Point,
  strokeWidth: number,
  minimumExtent: number,
): StrokeCommit | null {
  const deltaX = end[0] - start[0];
  const deltaY = end[1] - start[1];
  if (Math.hypot(deltaX, deltaY) < minimumExtent) return null;
  const half = strokeWidth / 2;
  // 读侧把路径从四边各内缩半个线宽，所以这里先把包围盒按线宽撑开。
  const width = Math.abs(deltaX) + strokeWidth;
  const height = Math.abs(deltaY) + strokeWidth;
  if (!(width > 0) || !(height > 0) || width > MAX_STROKE_EXTENT || height > MAX_STROKE_EXTENT) {
    return null;
  }
  const left = Math.min(start[0], end[0]) - half;
  const top = Math.max(start[1], end[1]) + half;
  // 方向位按局部坐标计：局部 y 向下，所以「世界 y 更大的起点」才是局部上方。
  const startRight = deltaX < 0;
  const startTop = deltaY < 0;
  const direction = startTop ? (startRight ? 1 : 0) : (startRight ? 2 : 3);
  return {
    tool,
    width,
    height,
    strokeWidth,
    geometry: encodeStrokeGeometry(STROKE_TOOL[tool], [], direction),
    centerX: left + width / 2,
    centerY: top - height / 2,
  };
}

function pointsBounds(points: readonly Point[]) {
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
  return { minX, minY, maxX, maxY };
}
