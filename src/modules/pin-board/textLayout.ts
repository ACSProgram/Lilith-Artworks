import {
  PIN_BOARD_NATIVE_TEXTURE_LIMIT,
  PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES,
} from "./texturePolicy";
import type { PinBoardTextItem } from "./types";

/** 与 Rust `pin_board::repository::TEXT_FONT_FAMILY` 保持一致。 */
export const PIN_BOARD_TEXT_FONT_FAMILY =
  '"Microsoft YaHei UI", "Segoe UI", sans-serif';
export const PIN_BOARD_TEXT_LINE_HEIGHT_RATIO = 1.3;
/** 导出倍率：文字按逻辑尺寸的固定倍数栅格化，避免屏幕字体小图被放大。 */
export const PIN_BOARD_TEXT_EXPORT_SCALE = 2;
export const PIN_BOARD_TEXT_MAX_DIMENSION = 8192;

export interface PinBoardTextMetrics {
  lines: string[];
  width: number;
  height: number;
  lineHeight: number;
}

export interface PinBoardTextRaster {
  width: number;
  height: number;
  rgba: Uint8ClampedArray;
}

function textLines(text: string): string[] {
  return text.replace(/\r\n?/g, "\n").split("\n");
}

function createCanvas(
  width: number,
  height: number,
): { canvas: HTMLCanvasElement; context: CanvasRenderingContext2D } {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("无法创建文字画布");
  return { canvas, context };
}

/** 度量只需 `measureText`：复用一块 1×1 画布，避免每次量字都新建 canvas。 */
let measureContext: CanvasRenderingContext2D | null = null;

function metricContext(): CanvasRenderingContext2D {
  measureContext ??= createCanvas(1, 1).context;
  return measureContext;
}

/** 屏幕显示与导出共用的字体度量：行宽取最长行，行高固定比例。 */
export function measurePinBoardText(text: string, fontSize: number): PinBoardTextMetrics {
  const safeSize = Number.isFinite(fontSize) ? Math.max(1, fontSize) : 1;
  const lines = textLines(text);
  const lineHeight = Math.ceil(safeSize * PIN_BOARD_TEXT_LINE_HEIGHT_RATIO);
  const context = metricContext();
  context.font = `${safeSize}px ${PIN_BOARD_TEXT_FONT_FAMILY}`;
  const width = Math.max(
    1,
    Math.ceil(Math.max(...lines.map((line) => context.measureText(line || " ").width))),
  );
  return {
    lines,
    width,
    height: Math.max(1, lineHeight * lines.length),
    lineHeight,
  };
}

function budgetedTextDimension(item: PinBoardTextItem): number {
  const long = Math.max(item.width, item.height);
  const short = Math.min(item.width, item.height);
  const ratio = short > 0 ? short / long : 1;
  const approximate = Math.floor(
    Math.sqrt(PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES / (4 * Math.max(ratio, 1e-6))),
  );
  let dimension = Math.max(1, Math.min(PIN_BOARD_TEXT_MAX_DIMENSION, approximate));
  // 与渲染器读取 RGBA 负载时的上限对齐：取整后的面积也不得超过预算。
  while (
    dimension > 1
    && textTextureRgbaBytes(item, dimension) > PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES
  ) {
    dimension -= 1;
  }
  return dimension;
}

/** 请求长边决定纹理清晰度；与位图不同，文字可以超过逻辑像素尺寸。 */
export function requestedTextTextureDimension(
  item: PinBoardTextItem,
  displayDimension: number,
  deviceLimit: number,
): number {
  const finiteDeviceLimit = Number.isFinite(deviceLimit)
    ? Math.max(1, Math.floor(deviceLimit))
    : PIN_BOARD_NATIVE_TEXTURE_LIMIT;
  const cap = Math.min(
    PIN_BOARD_NATIVE_TEXTURE_LIMIT,
    finiteDeviceLimit,
    budgetedTextDimension(item),
  );
  const desired = Math.max(256, displayDimension * 1.25);
  return Math.max(1, Math.round(Math.min(cap, desired)));
}

/** 按请求长边缩放后的实际纹理尺寸。 */
export function textTextureDimensions(
  item: PinBoardTextItem,
  maxDimension: number,
): [number, number] {
  const long = Math.max(item.width, item.height);
  const scale = maxDimension / Math.max(long, 1);
  return [
    Math.max(1, Math.min(maxDimension, Math.round(item.width * scale))),
    Math.max(1, Math.min(maxDimension, Math.round(item.height * scale))),
  ];
}

export function textTextureRgbaBytes(
  item: PinBoardTextItem,
  maxDimension: number,
): number {
  const [width, height] = textTextureDimensions(item, maxDimension);
  return width * height * 4;
}

export function textTextureResidentBytes(
  item: PinBoardTextItem,
  maxDimension: number,
): number {
  const [width, height] = textTextureDimensions(item, maxDimension);
  const bytesPerRow = Math.ceil((width * 4) / 256) * 256;
  return bytesPerRow * height;
}

interface DrawnText {
  canvas: HTMLCanvasElement;
  context: CanvasRenderingContext2D;
  width: number;
  height: number;
}

interface TextInk {
  ascent: number;
  descent: number;
}

/**
 * 取整个文本块的真实字形墨迹上下界。
 *
 * `measureText` 的宽度是行宽，`actualBoundingBoxAscent/Descent` 才是字形实际
 * 画到的范围；用后者定位基线，文字块才能在行盒里真正居中，也不会因为字形
 * 略高于 em 方框而被纹理顶边裁掉。
 */
function measureTextInk(
  context: CanvasRenderingContext2D,
  lines: string[],
  fontSize: number,
): TextInk {
  let ascent = 0;
  let descent = 0;
  for (const line of lines) {
    if (!line) continue;
    const measured = context.measureText(line);
    const lineAscent = measured.actualBoundingBoxAscent;
    const lineDescent = measured.actualBoundingBoxDescent;
    if (Number.isFinite(lineAscent)) ascent = Math.max(ascent, lineAscent);
    if (Number.isFinite(lineDescent)) descent = Math.max(descent, lineDescent);
  }
  // 仅空白行或旧环境缺少墨迹度量时退回经验值，保证基线仍然可用。
  if (!(ascent > 0)) return { ascent: fontSize * 0.8, descent: fontSize * 0.2 };
  return { ascent, descent: Number.isFinite(descent) && descent > 0 ? descent : 0 };
}

/** 按请求长边把文字画到离屏 canvas；显示纹理与导出共用这一段绘制逻辑。 */
function drawPinBoardText(item: PinBoardTextItem, maxDimension: number): DrawnText {
  const metrics = measurePinBoardText(item.text, item.fontSize);
  const [width, height] = textTextureDimensions(item, maxDimension);
  const scale = Math.min(width / metrics.width, height / metrics.height);
  const { canvas, context } = createCanvas(width, height);
  context.clearRect(0, 0, width, height);
  let fontSize = item.fontSize * scale;
  context.font = `${fontSize}px ${PIN_BOARD_TEXT_FONT_FAMILY}`;
  context.fillStyle = item.color;
  context.textAlign = item.align;
  context.textBaseline = "alphabetic";
  const x = item.align === "left" ? 0 : item.align === "right" ? width : width / 2;
  const lineHeight = metrics.lineHeight * scale;
  let ink = measureTextInk(context, metrics.lines, fontSize);
  // 极端字形（组合符号、表情等）可能高于行盒；等比缩小绘制字号，避免上下裁切。
  if (ink.ascent + ink.descent > lineHeight) {
    fontSize *= lineHeight / (ink.ascent + ink.descent);
    context.font = `${fontSize}px ${PIN_BOARD_TEXT_FONT_FAMILY}`;
    ink = measureTextInk(context, metrics.lines, fontSize);
  }
  // 旧实现用 textBaseline = "top"，1.3 倍行高的余量全留在下方，文字因此偏上且
  // 顶部越出纹理。这里把墨迹块在每行行盒里垂直居中，再按墨迹上界定位基线。
  const inkOffset = (lineHeight - ink.ascent - ink.descent) / 2;
  metrics.lines.forEach((line, index) => {
    context.fillText(line, x, index * lineHeight + inkOffset + ink.ascent);
  });
  return { canvas, context, width, height };
}

export function rasterizePinBoardText(
  item: PinBoardTextItem,
  maxDimension: number,
): PinBoardTextRaster {
  const { context, width, height } = drawPinBoardText(item, maxDimension);
  return {
    width,
    height,
    rgba: context.getImageData(0, 0, width, height).data,
  };
}

/**
 * 导出用的文字载荷：浏览器把它编码成 PNG 再交给原生端写盘。
 *
 * 原生端要以 RGBA 收同样的图需要跨 IPC 传数千万个数字（最长边 8192 时约
 * 64 MiB 像素），文案 PNG 则是几十到几百 KB。原生端只校验格式与尺寸后原子
 * 写盘，不再重新编码。
 */
export async function rasterizePinBoardTextPng(
  item: PinBoardTextItem,
  maxDimension: number,
): Promise<Uint8Array> {
  const { canvas } = drawPinBoardText(item, maxDimension);
  const blob = await new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((result) => {
      if (result) resolve(result);
      else reject(new Error("无法生成文字图片"));
    }, "image/png");
  });
  return new Uint8Array(await blob.arrayBuffer());
}

/**
 * 与 Rust 纹理读取一致的 RGBA 负载：`"RGBA"` + 小端宽高 + 像素。
 * 渲染器复用既有 `isRgbaPayload` 分支上传。
 */
export function textRgbaPayload(
  item: PinBoardTextItem,
  maxDimension: number,
): Uint8Array {
  const { width, height, rgba } = rasterizePinBoardText(item, maxDimension);
  const payload = new Uint8Array(12 + rgba.byteLength);
  payload[0] = 0x52;
  payload[1] = 0x47;
  payload[2] = 0x42;
  payload[3] = 0x41;
  const view = new DataView(payload.buffer);
  view.setUint32(4, width, true);
  view.setUint32(8, height, true);
  payload.set(rgba, 12);
  return payload;
}

/** 导出分辨率：逻辑长边 × 固定倍率，并受像素上限与 RGBA 预算约束。 */
export function exportTextMaxDimension(item: PinBoardTextItem): number {
  const long = Math.max(item.width, item.height);
  return Math.max(
    1,
    Math.min(
      PIN_BOARD_TEXT_MAX_DIMENSION,
      budgetedTextDimension(item),
      Math.round(long * PIN_BOARD_TEXT_EXPORT_SCALE),
    ),
  );
}
