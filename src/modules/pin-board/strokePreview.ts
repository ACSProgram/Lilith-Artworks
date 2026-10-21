/**
 * 绘制中的 DOM 2D 预览层：把当前草稿画到画布叠加层上，不进 GPU 缓冲。
 *
 * 坐标直接用世界单位：把画布变换设成「世界坐标 → 设备像素」之后，线宽与形状尺寸天然
 * 与落库后的渲染一致，不必两处各写一份换算。自由笔的压感半径与着色器共用
 * `strokePointRadius`，因此预览与落库不会出现「预览细、落库粗」。
 */

import { screenToWorld, viewportBounds, type Point, type Viewport } from "./geometry";
import { strokeDraftSamples, type StrokeDraft } from "./strokeDrawing";
import { strokePointRadius } from "./strokeRender";

/** 一次指针事件换算出的笔迹采样：世界坐标 + 压感 + 指针类型。 */
export interface PointerStrokeSample {
  point: Point;
  pressure: number;
  pointerType: string;
}

/**
 * 一次指针事件里的全部采样点。
 *
 * Chromium 会把 `pointermove` 合并到约 60 Hz，被合并掉的中间点只在
 * `getCoalescedEvents()` 里；不可用（旧内核、测试环境）或返回空数组时退回事件本身。
 * 一次事件内的所有点共用同一个画布矩形，不必逐点测量布局。
 */
export function coalescedStrokeSamples(
  event: PointerEvent,
  rect: DOMRect,
  viewport: Viewport,
): PointerStrokeSample[] {
  const coalesced = typeof event.getCoalescedEvents === "function"
    ? event.getCoalescedEvents()
    : [];
  const events = coalesced.length > 0 ? coalesced : [event];
  return events.map((entry) => ({
    point: screenToWorld(entry.clientX, entry.clientY, rect, viewport),
    pressure: entry.pressure,
    pointerType: entry.pointerType ?? "",
  }));
}

/** 清空预览层（含画布尺寸归一），没有叠加层时静默返回。 */
export function clearStrokePreview(overlay: HTMLCanvasElement | null): void {
  const context = overlay?.getContext("2d");
  if (!overlay || !context) return;
  context.setTransform(1, 0, 0, 1, 0, 0);
  context.clearRect(0, 0, overlay.width, overlay.height);
}

export interface StrokePreviewOptions {
  overlay: HTMLCanvasElement;
  canvas: HTMLCanvasElement;
  viewport: Viewport;
  draft: StrokeDraft | null;
  color: string;
  width: number;
}

/**
 * 把当前草稿画到 DOM 叠加层上。画布尺寸按 CSS 尺寸 × devicePixelRatio 同步，
 * 变换设成「世界 → 设备像素」后直接按世界坐标画。
 */
export function drawStrokePreview(options: StrokePreviewOptions): void {
  const { overlay, canvas, viewport, draft, color, width } = options;
  const context = overlay.getContext("2d");
  if (!context) return;
  const rect = canvas.getBoundingClientRect();
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
  if (!draft) return;
  const bounds = viewportBounds(viewport, cssWidth, cssHeight);
  const scale = pixelWidth / Math.max(bounds.maxX - bounds.minX, 1e-9);
  context.setTransform(scale, 0, 0, -scale, -bounds.minX * scale, bounds.maxY * scale);
  context.strokeStyle = color;
  context.fillStyle = color;
  context.lineWidth = width;
  context.lineJoin = "round";
  context.lineCap = "round";
  const samples = strokeDraftSamples(draft);
  // 自由笔每个指针事件都重画一遍：直接读采样数组，不再复制成一份点数组。
  if (draft.tool === "pen") {
    if (samples.length < 2) return;
    // 数位笔的预览按压感画成变宽墨迹：与落库判定同一条件（指针是笔**且**确实
    // 收到过非零压感），否则会出现「预览细、落库粗」的跳变。鼠标仍画等宽折线。
    if (draft.pen && samples.some((sample) => sample.pressure > 0)) {
      strokePreviewInk(context, samples, width);
      return;
    }
    context.beginPath();
    context.moveTo(samples[0].point[0], samples[0].point[1]);
    for (let index = 1; index < samples.length; index += 1) {
      context.lineTo(samples[index].point[0], samples[index].point[1]);
    }
    context.stroke();
    return;
  }
  const [start, end] = [samples[0].point, samples[1].point];
  switch (draft.tool) {
    case "line":
      strokePreviewSegment(context, start, end);
      return;
    case "arrow": {
      const deltaX = end[0] - start[0];
      const deltaY = end[1] - start[1];
      const length = Math.hypot(deltaX, deltaY);
      if (length < 1e-6) return;
      const unitX = deltaX / length;
      const unitY = deltaY / length;
      const headLength = Math.min(length, Math.max(width * 4, 1e-4));
      const halfBase = headLength * 0.5;
      const base: Point = [end[0] - unitX * headLength, end[1] - unitY * headLength];
      // 杆止于三角形底边，与渲染层一致：圆头藏在箭头里，不会从箭尖后面冒出来。
      strokePreviewSegment(context, start, base);
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

function strokePreviewSegment(
  context: CanvasRenderingContext2D,
  start: Point,
  end: Point,
) {
  context.beginPath();
  context.moveTo(start[0], start[1]);
  context.lineTo(end[0], end[1]);
  context.stroke();
}

/**
 * 按压感把一段草稿画成变宽的墨迹：沿中心线按每点的半径向两侧偏移出一条闭合
 * 轮廓再一次填充，而不是逐段 `stroke()`——后者在几百个点上就是几百次绘制调用，
 * 每次 `pointermove` 重画一遍会拖慢预览。
 */
function strokePreviewInk(
  context: CanvasRenderingContext2D,
  samples: readonly { point: Point; pressure: number }[],
  strokeWidth: number,
) {
  if (samples.length < 2) return;
  // 与着色器共用同一个半径公式，预览与落库不会各写一份而慢慢漂移。
  const radii = samples.map((sample) => (
    strokePointRadius(strokeWidth, sample.pressure, true)
  ));
  const left: Point[] = [];
  const right: Point[] = [];
  for (let index = 0; index < samples.length; index += 1) {
    const previous = samples[Math.max(0, index - 1)].point;
    const next = samples[Math.min(samples.length - 1, index + 1)].point;
    let deltaX = next[0] - previous[0];
    let deltaY = next[1] - previous[1];
    const length = Math.hypot(deltaX, deltaY);
    if (!(length > 0)) {
      deltaX = 1;
      deltaY = 0;
    } else {
      deltaX /= length;
      deltaY /= length;
    }
    const center = samples[index].point;
    const radius = radii[index];
    left.push([center[0] - deltaY * radius, center[1] + deltaX * radius]);
    right.push([center[0] + deltaY * radius, center[1] - deltaX * radius]);
  }
  context.beginPath();
  context.moveTo(left[0][0], left[0][1]);
  for (let index = 1; index < left.length; index += 1) {
    context.lineTo(left[index][0], left[index][1]);
  }
  for (let index = right.length - 1; index >= 0; index -= 1) {
    context.lineTo(right[index][0], right[index][1]);
  }
  context.closePath();
  context.fill();
}
