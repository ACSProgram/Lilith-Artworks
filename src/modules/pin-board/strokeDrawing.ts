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
 *
 * 自由笔在落库前还要过一遍 `strokeSampling.ts` 的采样管道（抽稀、光滑、按弧长重采样、
 * 压感平滑与两端 taper）；草稿本身保留原始采样，预览直接画它们，因此预览与落库的
 * 形状只差那一次光滑（亚像素级）。
 */

import type { Point } from "./geometry";
import {
  MAX_STROKE_EXTENT,
  MAX_STROKE_POINTS,
  STROKE_TOOL,
  encodeStrokeGeometry,
  type PinBoardStrokeTool,
} from "./strokeGeometry";
import {
  normalisePenSamples,
  type PressureSample,
} from "./strokeSampling";

/** 可绘制工具；`select` 不在其中，它沿用原有的选中/框选/变换交互。 */
export type StrokeToolName = "pen" | "line" | "arrow" | "rect" | "ellipse";
/** 全部工具：`eraser` 不产生几何，但和绘制工具一样独占指针（不选中、不框选）。 */
export type PinBoardTool = "select" | StrokeToolName | "eraser";

/** 标注的默认颜色取偏红的高对比色：在多数参考图上都能看清，又不至于像纯红那样刺眼。 */
export const DEFAULT_STROKE_COLOR = "#e5484d";
export const DEFAULT_STROKE_WIDTH = 6;
/**
 * 线宽是世界单位（与图片像素同量级）。上限取 500：大幅面画布上做粗标注够用，
 * 仍比原生端与数据库 CHECK 的 1000 保守一档，不必为此改 schema。
 */
export const MIN_STROKE_WIDTH = 1;
export const MAX_STROKE_WIDTH = 500;
/**
 * 线宽滑条的内部刻度数。滑条位置与线宽之间走指数映射：500 倍的范围里，线宽 1–20
 * 这一段最常调，必须占到足够的行程，而大线宽那一段差几十个单位也看不出来。
 */
export const STROKE_WIDTH_SLIDER_STEPS = 1000;
/**
 * 最小拖拽距离（CSS 像素）。低于它按「误点」处理：单击不留下一个看不见的条目，
 * 形状也拿不到退化的包围盒。
 */
export const MIN_STROKE_DRAG_CSS_PIXELS = 3;

/** 同一个点重复到达时的合并阈值（世界单位）：指针事件常带完全相同的坐标。 */
const MIN_SAMPLE_DISTANCE = 1e-6;

/**
 * 工具的键位、界面名与顺序：快捷键分发与标注工具条共用这一份，提示里的字母因此
 * 不可能与真正生效的键位漂移。
 */
export const PIN_BOARD_TOOL_META: ReadonlyArray<{
  tool: PinBoardTool;
  label: string;
  shortcut: string;
}> = [
  { tool: "select", label: "选择", shortcut: "V" },
  { tool: "pen", label: "画笔", shortcut: "P" },
  { tool: "eraser", label: "橡皮", shortcut: "E" },
  { tool: "line", label: "直线", shortcut: "L" },
  { tool: "arrow", label: "箭头", shortcut: "A" },
  { tool: "rect", label: "矩形", shortcut: "R" },
  { tool: "ellipse", label: "椭圆", shortcut: "O" },
];

const TOOL_BY_KEY: Readonly<Record<string, PinBoardTool>> = Object.fromEntries(
  PIN_BOARD_TOOL_META.map((entry) => [entry.shortcut.toLowerCase(), entry.tool]),
);

/**
 * 是否会产生几何（笔 / 直线 / 箭头 / 矩形 / 椭圆）。
 *
 * 只有这一类才走「草稿 → 编码 → 落库」；擦拭同样占用指针，却一条几何也不写，
 * 因此必须与它分开——把橡皮当成笔会让绘制分支去编码一个不存在的草稿。
 */
export function isStrokeTool(tool: PinBoardTool): tool is StrokeToolName {
  return tool !== "select" && tool !== "eraser";
}

/**
 * 是否由工具独占指针（绘制或擦除）。这两类工具下 `pointerdown` 不进选中/框选分支，
 * 也不该带着上一轮的选中框。
 */
export function isDrawingTool(tool: PinBoardTool): boolean {
  return tool !== "select";
}

/** 前端工具名 → 几何 BLOB 里的工具编号。 */
export function strokeToolId(tool: StrokeToolName): PinBoardStrokeTool {
  return STROKE_TOOL[tool];
}

/** 单键工具快捷键：键位只在 `PIN_BOARD_TOOL_META` 里写一份，这里按小写归一后查表。 */
export function toolForShortcut(key: string): PinBoardTool | null {
  return TOOL_BY_KEY[key.toLowerCase()] ?? null;
}

/** 线宽收敛到输入框与原生端都接受的范围；非有限值回落到默认线宽。 */
export function clampStrokeWidth(width: number): number {
  if (!Number.isFinite(width)) return DEFAULT_STROKE_WIDTH;
  return Math.min(MAX_STROKE_WIDTH, Math.max(MIN_STROKE_WIDTH, width));
}

/** 线宽范围比，指数映射的底数。 */
const STROKE_WIDTH_RANGE_RATIO = MAX_STROKE_WIDTH / MIN_STROKE_WIDTH;

/**
 * 滑条位置的压缩指数。位置先做 `t^0.75` 再进指数映射：纯指数下每前进 10% 线宽都乘
 * 1.86 倍，粗线端（右半段）因此窜得太快；压缩后右半段每 10% 只乘约 1.6 倍，200–500
 * 那一段的行程从 15% 放宽到 19%。代价是细线端略陡，1–20 仍占约 38% 行程，一档一档
 * 调够用。
 */
const STROKE_WIDTH_SLIDER_CURVE = 0.75;

/**
 * 滑条位置（0–`STROKE_WIDTH_SLIDER_STEPS`）→ 线宽。
 *
 * 指数映射：位置每前进 10%，线宽乘 `RANGE_RATIO ^ 0.1`（本范围约 1.86 倍），再按
 * `t^0.75` 把行程往细线端挪一点（见 `STROKE_WIDTH_SLIDER_CURVE`）——实际效果是细线
 * 端每 10% 乘约 1.9 倍、粗线端约 1.6 倍。1–20 这段因此占到约 38% 的行程，可以一档一
 * 档地调；200 以上那一段占最后两成，反正那里差几十个单位也看不出来。取整后再夹一次
 * 范围，两端始终是 1 与上限。
 */
export function strokeWidthFromSlider(position: number): number {
  if (!Number.isFinite(position)) return DEFAULT_STROKE_WIDTH;
  const track = Math.min(1, Math.max(0, position / STROKE_WIDTH_SLIDER_STEPS));
  const curved = track ** STROKE_WIDTH_SLIDER_CURVE;
  return clampStrokeWidth(Math.round(MIN_STROKE_WIDTH * STROKE_WIDTH_RANGE_RATIO ** curved));
}

/**
 * 线宽 → 滑条位置（`strokeWidthFromSlider` 的反函数，供滑条显示）。
 *
 * 反解分两步：先取对数得到压缩后的位置，再开 `1 / STROKE_WIDTH_SLIDER_CURVE` 次方还原。
 */
export function strokeWidthToSlider(width: number): number {
  const value = clampStrokeWidth(width);
  const curved = Math.log(value / MIN_STROKE_WIDTH) / Math.log(STROKE_WIDTH_RANGE_RATIO);
  const position = Math.min(1, Math.max(0, curved)) ** (1 / STROKE_WIDTH_SLIDER_CURVE);
  return Math.round(position * STROKE_WIDTH_SLIDER_STEPS);
}

/** 一次绘制的草稿：世界坐标，与视口无关（缩放/平移不影响已采样的点）。 */
export interface StrokeDraft {
  readonly tool: StrokeToolName;
  /** 自由笔的全部采样点（含压感）；形状只保留起点。 */
  readonly samples: PressureSample[];
  /** 当前末点（未经 Shift 约束）；形状与 Shift 下的自由笔都从这里取末端。 */
  end: Point;
  /** 末点对应的压感：Shift 直线只保留首尾两点，末端也要带上它。 */
  endPressure: number;
  /** 最后一次指针事件的 Shift 状态。 */
  shift: boolean;
  /** 这次拖拽里是否出现过数位笔：决定落库时是否写压感标志。 */
  pen: boolean;
}

/** 指针事件里与笔迹有关的附加信息；渲染器从 `PointerEvent` 取。 */
export interface StrokeInput {
  pressure?: number;
  pointerType?: string;
}

export function beginStrokeDraft(
  tool: StrokeToolName,
  point: Point,
  input: StrokeInput = {},
): StrokeDraft {
  const pressure = normalisePressure(input.pressure);
  return {
    tool,
    samples: [{ point: [point[0], point[1]], pressure }],
    end: [point[0], point[1]],
    endPressure: pressure,
    shift: false,
    pen: input.pointerType === "pen",
  };
}

/**
 * 追加一个指针位置。
 *
 * 自由笔在按住 Shift 时不再累计点：那段时间的轨迹就是一条从起点出发的直线，末端由
 * `end` 给出；松开 Shift 后继续从起点往后累计（与常见绘图软件一致）。
 */
export function extendStrokeDraft(
  draft: StrokeDraft,
  point: Point,
  shift: boolean,
  input: StrokeInput = {},
): void {
  draft.end = [point[0], point[1]];
  draft.endPressure = normalisePressure(input.pressure);
  draft.shift = shift;
  if (input.pointerType === "pen") draft.pen = true;
  if (draft.tool !== "pen" || shift) return;
  const last = draft.samples[draft.samples.length - 1].point;
  if (Math.hypot(point[0] - last[0], point[1] - last[1]) < MIN_SAMPLE_DISTANCE) {
    // 位置没变，但压感可能变了（按住不动时压感仍会漂移）：更新末点而不是丢样本。
    draft.samples[draft.samples.length - 1] = {
      point: [last[0], last[1]],
      pressure: draft.endPressure,
    };
    return;
  }
  draft.samples.push({ point: [point[0], point[1]], pressure: draft.endPressure });
}

/** 草稿的有效点序列：形状为 [起点, 约束后的末点]，自由笔为全部采样点。 */
export function strokeDraftPoints(draft: StrokeDraft): readonly Point[] {
  return strokeDraftSamples(draft).map((sample) => sample.point);
}

/** 草稿的有效采样（带压感），与 `strokeDraftPoints` 同一顺序。 */
export function strokeDraftSamples(draft: StrokeDraft): readonly PressureSample[] {
  const start = draft.samples[0];
  if (draft.tool === "pen") {
    if (!draft.shift) return draft.samples;
    return [
      { point: [start.point[0], start.point[1]], pressure: start.pressure },
      { point: [draft.end[0], draft.end[1]], pressure: draft.endPressure },
    ];
  }
  return [
    start,
    {
      point: constrainShapeEnd(draft.tool, start.point, draft.end, draft.shift),
      pressure: 0,
    },
  ];
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
 * `minimumExtent` 是拖拽距离下限（世界单位），由渲染器按当前缩放换算成像素；
 * `pixelsPerWorldUnit` 是绘制时的缩放，采样管道的容差、步长与 taper 都按它换算——
 * 用户在哪个缩放下画，存下来的点密度就按那个缩放定。
 */
export function resolveStrokeDraft(
  draft: StrokeDraft,
  strokeWidth: number,
  minimumExtent = 0,
  pixelsPerWorldUnit = 1,
): StrokeCommit | null {
  const width = clampStrokeWidth(strokeWidth);
  const samples = strokeDraftSamples(draft);
  if (draft.tool === "pen") {
    return penCommit(
      samples,
      width,
      minimumExtent,
      Math.max(pixelsPerWorldUnit, 1e-6),
      draft.pen && samples.some((sample) => sample.pressure > 0),
    );
  }
  return shapeCommit(draft.tool, samples[0].point, samples[1].point, width, minimumExtent);
}

/**
 * 自由笔的落库几何：先过采样管道（抽稀 → 光滑 → 重采样 → 压感平滑与 taper），
 * 再按墨迹外框归一化。点数上限由管道兜住，这里不会再被拒绝。
 */
function penCommit(
  samples: readonly PressureSample[],
  strokeWidth: number,
  minimumExtent: number,
  pixelsPerWorldUnit: number,
  usePressure: boolean,
): StrokeCommit | null {
  // 单点画不出线段：与其留一个看不见的条目，不如按取消处理。
  if (samples.length < 2) return null;
  const points = normalisePenSamples(samples, {
    pixelsPerWorldUnit,
    strokeWidth,
    usePressure,
    maxPoints: MAX_STROKE_POINTS,
  });
  if (points.length < 2) return null;
  const bounds = pointsBounds(points.map((sample) => sample.point));
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
  const normalized = points.map(({ point, pressure }) => ({
    u: (point[0] - left) / width,
    // 局部 y 向下：顶边对应世界坐标的最大 y。
    v: (top - point[1]) / height,
    pressure,
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

/** 压感收敛到 0–1；缺失或非有限值按 0（设备没给数据）处理。 */
function normalisePressure(pressure: number | undefined): number {
  if (pressure === undefined || !Number.isFinite(pressure)) return 0;
  return Math.min(1, Math.max(0, pressure));
}
